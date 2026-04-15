// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(dead_code)]
//
// X11 atom table.
//
// An atom in X11 is a 32-bit numeric identifier for a short string, used
// pervasively for window property names (WM_NAME, _NET_WM_NAME),
// property types (STRING, UTF8_STRING, CARDINAL), selection names
// (PRIMARY, CLIPBOARD), and so on.  The atom table is a single global
// bidirectional map kept inside the server; clients intern names as
// they need them and use the resulting u32 wherever an atom is expected.
//
// The X11 spec reserves ids 1..=68 for a fixed set of predefined atoms
// (defined in Xatom.h).  We pre-seed those at server startup so they
// have their canonical ids and clients that use them without an
// explicit InternAtom (rare but legal) get the expected ids.
//
// Dynamically interned atoms get ids starting at 69 and increment by
// one for each new name.

// We deliberately use BTreeMap rather than HashMap: HashMap on musl
// targets pulls in thread-local RandomState initialization that crashes
// before `main` on some static-musl builds.  BTreeMap has no RNG state,
// costs us log(N) instead of O(1) lookups, and N is bounded by the
// number of unique atom names a session interns (typically <200).
use std::collections::BTreeMap;

pub struct AtomTable {
    /// Name by id.  Index 0 is reserved (atom 0 is "None"), so entries
    /// start at index 1.  `by_id[0]` holds `None`, `by_id[1]` holds
    /// `Some("PRIMARY")`, etc.
    by_id: Vec<Option<String>>,
    /// Inverse map (ordered; BTreeMap avoids HashMap's RNG init).
    by_name: BTreeMap<String, u32>,
    /// Next id to assign to a newly interned atom.
    next: u32,
}

/// The 68 predefined atoms defined in <X11/Xatom.h>.  Index in this
/// slice + 1 is the canonical atom id.
pub const PREDEFINED: &[&str] = &[
    "PRIMARY",            // 1
    "SECONDARY",          // 2
    "ARC",                // 3
    "ATOM",               // 4
    "BITMAP",             // 5
    "CARDINAL",           // 6
    "COLORMAP",           // 7
    "CURSOR",             // 8
    "CUT_BUFFER0",        // 9
    "CUT_BUFFER1",        // 10
    "CUT_BUFFER2",        // 11
    "CUT_BUFFER3",        // 12
    "CUT_BUFFER4",        // 13
    "CUT_BUFFER5",        // 14
    "CUT_BUFFER6",        // 15
    "CUT_BUFFER7",        // 16
    "DRAWABLE",           // 17
    "FONT",               // 18
    "INTEGER",            // 19
    "PIXMAP",             // 20
    "POINT",              // 21
    "RECTANGLE",          // 22
    "RESOURCE_MANAGER",   // 23
    "RGB_COLOR_MAP",      // 24
    "RGB_BEST_MAP",       // 25
    "RGB_BLUE_MAP",       // 26
    "RGB_DEFAULT_MAP",    // 27
    "RGB_GRAY_MAP",       // 28
    "RGB_GREEN_MAP",      // 29
    "RGB_RED_MAP",        // 30
    "STRING",             // 31
    "VISUALID",           // 32
    "WINDOW",             // 33
    "WM_COMMAND",         // 34
    "WM_HINTS",           // 35
    "WM_CLIENT_MACHINE",  // 36
    "WM_ICON_NAME",       // 37
    "WM_ICON_SIZE",       // 38
    "WM_NAME",            // 39
    "WM_NORMAL_HINTS",    // 40
    "WM_SIZE_HINTS",      // 41
    "WM_ZOOM_HINTS",      // 42
    "MIN_SPACE",          // 43
    "NORM_SPACE",         // 44
    "MAX_SPACE",          // 45
    "END_SPACE",          // 46
    "SUPERSCRIPT_X",      // 47
    "SUPERSCRIPT_Y",      // 48
    "SUBSCRIPT_X",        // 49
    "SUBSCRIPT_Y",        // 50
    "UNDERLINE_POSITION", // 51
    "UNDERLINE_THICKNESS",// 52
    "STRIKEOUT_ASCENT",   // 53
    "STRIKEOUT_DESCENT",  // 54
    "ITALIC_ANGLE",       // 55
    "X_HEIGHT",           // 56
    "QUAD_WIDTH",         // 57
    "WEIGHT",             // 58
    "POINT_SIZE",         // 59
    "RESOLUTION",         // 60
    "COPYRIGHT",          // 61
    "NOTICE",             // 62
    "FONT_NAME",          // 63
    "FAMILY_NAME",        // 64
    "FULL_NAME",          // 65
    "CAP_HEIGHT",         // 66
    "WM_CLASS",           // 67
    "WM_TRANSIENT_FOR",   // 68
];

/// Canonical ids for atoms we care about checking by constant.
pub mod id {
    pub const NONE:                u32 = 0;
    pub const PRIMARY:             u32 = 1;
    pub const SECONDARY:           u32 = 2;
    pub const ATOM:                u32 = 4;
    pub const CARDINAL:            u32 = 6;
    pub const INTEGER:             u32 = 19;
    pub const STRING:              u32 = 31;
    pub const VISUALID:            u32 = 32;
    pub const WINDOW:              u32 = 33;
    pub const WM_COMMAND:          u32 = 34;
    pub const WM_HINTS:            u32 = 35;
    pub const WM_NAME:             u32 = 39;
    pub const WM_NORMAL_HINTS:     u32 = 40;
    pub const WM_SIZE_HINTS:       u32 = 41;
    pub const WM_CLASS:            u32 = 67;
    pub const WM_TRANSIENT_FOR:    u32 = 68;
}

impl AtomTable {
    pub fn new() -> Self {
        let mut t = AtomTable {
            by_id: vec![None],  // slot 0 = None atom
            by_name: BTreeMap::new(),
            next: 1,
        };
        for name in PREDEFINED {
            let id = t.next;
            t.by_id.push(Some(String::from(*name)));
            t.by_name.insert(String::from(*name), id);
            t.next += 1;
        }
        debug_assert_eq!(t.next, 69);
        t
    }

    /// Intern `name`, returning the existing id if already interned or
    /// creating a new id (starting at 69 for dynamic atoms).
    ///
    /// If `only_if_exists` is true and the name is not already present,
    /// returns `None` (the protocol wire value is 0, which is `NONE`).
    pub fn intern(&mut self, name: &str, only_if_exists: bool) -> Option<u32> {
        if let Some(&id) = self.by_name.get(name) {
            return Some(id);
        }
        if only_if_exists {
            return None;
        }
        let id = self.next;
        self.by_id.push(Some(String::from(name)));
        self.by_name.insert(String::from(name), id);
        self.next += 1;
        Some(id)
    }

    /// Look up the name for an id.  Returns `None` for id 0 or unknown ids.
    pub fn name(&self, id: u32) -> Option<&str> {
        self.by_id
            .get(id as usize)
            .and_then(|o| o.as_deref())
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn predefined_ids() {
        let t = AtomTable::new();
        assert_eq!(t.name(1),  Some("PRIMARY"));
        assert_eq!(t.name(39), Some("WM_NAME"));
        assert_eq!(t.name(67), Some("WM_CLASS"));
        assert_eq!(t.name(68), Some("WM_TRANSIENT_FOR"));
        assert_eq!(t.name(0),  None);
        assert_eq!(t.name(69), None);   // no dynamic atoms yet
    }

    #[test]
    fn intern_roundtrip() {
        let mut t = AtomTable::new();
        let a1 = t.intern("UTF8_STRING", false).unwrap();
        let a2 = t.intern("UTF8_STRING", false).unwrap();
        assert_eq!(a1, a2);
        assert_eq!(a1, 69); // first dynamic id
        assert_eq!(t.name(a1), Some("UTF8_STRING"));
    }

    #[test]
    fn intern_predefined_returns_canonical() {
        let mut t = AtomTable::new();
        let id = t.intern("WM_NAME", false).unwrap();
        assert_eq!(id, 39);
        assert_eq!(t.len(), 68); // no new entry
    }

    #[test]
    fn only_if_exists() {
        let mut t = AtomTable::new();
        assert_eq!(t.intern("NONEXISTENT", true), None);
        assert_eq!(t.intern("WM_NAME", true), Some(39));
    }

    #[test]
    fn fresh_dynamic_ids_monotonic() {
        let mut t = AtomTable::new();
        let a = t.intern("FOO", false).unwrap();
        let b = t.intern("BAR", false).unwrap();
        assert_eq!(a, 69);
        assert_eq!(b, 70);
    }
}
