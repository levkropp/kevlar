// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(dead_code)]
//
// Resource ID (XID) allocation and the global resource map.
//
// In X11, every drawable, graphics context, font, colormap, cursor, and
// pixmap is identified by a 32-bit XID.  Clients allocate IDs within a
// per-client range (base + 0..mask).  The server records ownership so
// that a client can only refer to its own resources, and so that when
// the client disconnects all its resources can be reaped.
//
// We use the same scheme the `setup.rs` module advertises in the
// ConnectionSetup reply:
//
//     resource-id-base = client_id << 21
//     resource-id-mask = 0x001F_FFFF   (21 bits = 2M ids per client)
//
// Client 1 allocates ids in `0x00200000..=0x003FFFFF`, client 2 in
// `0x00400000..=0x005FFFFF`, and so on.  The root window uses a fixed
// id `0x00000020` in the server's own namespace (below any client
// range), so clients see it but cannot allocate over it.

use crate::font::FontRef;
use crate::gc::Gc;
use crate::pixmap::Pixmap;
use crate::region::Region;
use crate::render_ext::{GlyphSet, Picture};
use crate::setup;
use crate::window::Window;

/// Every resource in the server is one of these variants.  Phase 4
/// adds GCs and colormaps to the set.  Phase 5 adds Pixmap.
#[derive(Debug)]
pub enum Resource {
    Window(Window),
    Gc(Gc),
    /// Colormap is just an id on a TrueColor visual (no lookup table).
    /// We store the visual it was created against for future checks.
    Colormap { visual: u32 },
    Pixmap(Pixmap),
    Font(FontRef),
    Picture(Picture),
    GlyphSet(GlyphSet),
    Region(Region),
    // Phase 5: Cursor (stub only)
}

impl Resource {
    pub fn kind(&self) -> &'static str {
        match self {
            Resource::Window(_)     => "Window",
            Resource::Gc(_)         => "Gc",
            Resource::Colormap { .. } => "Colormap",
            Resource::Pixmap(_)     => "Pixmap",
            Resource::Font(_)       => "Font",
            Resource::Picture(_)    => "Picture",
            Resource::GlyphSet(_)   => "GlyphSet",
            Resource::Region(_)     => "Region",
        }
    }

    pub fn as_window(&self) -> Option<&Window> {
        match self { Resource::Window(w) => Some(w), _ => None }
    }

    pub fn as_window_mut(&mut self) -> Option<&mut Window> {
        match self { Resource::Window(w) => Some(w), _ => None }
    }

    pub fn as_gc(&self) -> Option<&Gc> {
        match self { Resource::Gc(g) => Some(g), _ => None }
    }

    pub fn as_gc_mut(&mut self) -> Option<&mut Gc> {
        match self { Resource::Gc(g) => Some(g), _ => None }
    }

    pub fn as_pixmap(&self) -> Option<&Pixmap> {
        match self { Resource::Pixmap(p) => Some(p), _ => None }
    }

    pub fn as_pixmap_mut(&mut self) -> Option<&mut Pixmap> {
        match self { Resource::Pixmap(p) => Some(p), _ => None }
    }

    pub fn as_font(&self) -> Option<&FontRef> {
        match self { Resource::Font(f) => Some(f), _ => None }
    }

    pub fn as_picture(&self) -> Option<&Picture> {
        match self { Resource::Picture(p) => Some(p), _ => None }
    }

    pub fn as_picture_mut(&mut self) -> Option<&mut Picture> {
        match self { Resource::Picture(p) => Some(p), _ => None }
    }

    pub fn as_glyph_set(&self) -> Option<&GlyphSet> {
        match self { Resource::GlyphSet(g) => Some(g), _ => None }
    }

    pub fn as_glyph_set_mut(&mut self) -> Option<&mut GlyphSet> {
        match self { Resource::GlyphSet(g) => Some(g), _ => None }
    }

    pub fn as_region(&self) -> Option<&Region> {
        match self { Resource::Region(r) => Some(r), _ => None }
    }

    pub fn as_region_mut(&mut self) -> Option<&mut Region> {
        match self { Resource::Region(r) => Some(r), _ => None }
    }
}

/// Per-resource ownership metadata.  Used for cleanup on client exit.
pub struct ResourceEntry {
    /// The client that allocated the id, or `0` for server-owned
    /// resources (currently only the root window).
    pub owner: u32,
    pub data: Resource,
}

/// The global id → resource map.  Keyed by XID.  Lookups are O(log N)
/// via BTreeMap (avoids the static-musl HashMap crash from blog 163
/// and keeps allocation predictable).
pub struct ResourceMap {
    // Primary store.
    entries: std::collections::BTreeMap<u32, ResourceEntry>,
}

impl ResourceMap {
    pub fn new() -> Self {
        ResourceMap { entries: std::collections::BTreeMap::new() }
    }

    pub fn insert(&mut self, id: u32, owner: u32, res: Resource) {
        self.entries.insert(id, ResourceEntry { owner, data: res });
    }

    pub fn get(&self, id: u32) -> Option<&ResourceEntry> {
        self.entries.get(&id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut ResourceEntry> {
        self.entries.get_mut(&id)
    }

    pub fn window(&self, id: u32) -> Option<&Window> {
        self.get(id).and_then(|e| e.data.as_window())
    }

    pub fn window_mut(&mut self, id: u32) -> Option<&mut Window> {
        self.get_mut(id).and_then(|e| e.data.as_window_mut())
    }

    pub fn gc(&self, id: u32) -> Option<&Gc> {
        self.get(id).and_then(|e| e.data.as_gc())
    }

    pub fn gc_mut(&mut self, id: u32) -> Option<&mut Gc> {
        self.get_mut(id).and_then(|e| e.data.as_gc_mut())
    }

    pub fn pixmap(&self, id: u32) -> Option<&Pixmap> {
        self.get(id).and_then(|e| e.data.as_pixmap())
    }

    pub fn pixmap_mut(&mut self, id: u32) -> Option<&mut Pixmap> {
        self.get_mut(id).and_then(|e| e.data.as_pixmap_mut())
    }

    pub fn font(&self, id: u32) -> Option<&FontRef> {
        self.get(id).and_then(|e| e.data.as_font())
    }

    pub fn picture(&self, id: u32) -> Option<&Picture> {
        self.get(id).and_then(|e| e.data.as_picture())
    }

    pub fn picture_mut(&mut self, id: u32) -> Option<&mut Picture> {
        self.get_mut(id).and_then(|e| e.data.as_picture_mut())
    }

    pub fn glyph_set(&self, id: u32) -> Option<&GlyphSet> {
        self.get(id).and_then(|e| e.data.as_glyph_set())
    }

    pub fn glyph_set_mut(&mut self, id: u32) -> Option<&mut GlyphSet> {
        self.get_mut(id).and_then(|e| e.data.as_glyph_set_mut())
    }

    pub fn region(&self, id: u32) -> Option<&Region> {
        self.get(id).and_then(|e| e.data.as_region())
    }

    pub fn region_mut(&mut self, id: u32) -> Option<&mut Region> {
        self.get_mut(id).and_then(|e| e.data.as_region_mut())
    }

    pub fn remove(&mut self, id: u32) -> Option<ResourceEntry> {
        self.entries.remove(&id)
    }

    /// Iterate all resources owned by the given client id.
    pub fn iter_owned_by(&self, owner: u32) -> impl Iterator<Item = (u32, &ResourceEntry)> {
        self.entries
            .iter()
            .filter_map(move |(id, e)| if e.owner == owner { Some((*id, e)) } else { None })
    }

    /// Drop every resource owned by the given client.  Returns the list
    /// of removed XIDs (so callers can propagate DestroyNotify events).
    pub fn reap_client(&mut self, owner: u32) -> Vec<u32> {
        let to_remove: Vec<u32> = self
            .entries
            .iter()
            .filter_map(|(id, e)| if e.owner == owner { Some(*id) } else { None })
            .collect();
        for id in &to_remove {
            self.entries.remove(id);
        }
        to_remove
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Validate that `id` belongs to `owner` (or is the root window, which
/// is shared).  Returns the resource entry on success, or `None` for
/// BadWindow/BadPixmap/etc. errors.
pub fn resource_belongs_to(id: u32, owner: u32) -> bool {
    if id == setup::ROOT_WINDOW_ID {
        return true;
    }
    let high = id >> 21;
    high == owner
}
