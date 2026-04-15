// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(dead_code)]
//
// Hardcoded US-QWERTY keymap, X11-style.
//
// X11 uses keycodes 8..=255 (8 is the minimum legal keycode per the
// spec).  Modifier levels 0 and 1 are the unshifted and shifted
// keysyms.  The classic PC/AT to X11 keycode mapping shifts the Linux
// evdev scancode up by 8: Linux `KEY_ESC = 1` → X11 `keycode 9`.
//
// Keysym values follow X11R6 `keysymdef.h` (low Latin-1 codepoints for
// printable characters).  Only the subset twm/xterm actually consult
// is populated — unknowns report `NoSymbol (0)` which xterm then
// silently ignores.
//
// If a real Kevlar keyboard device ever produces evdev scancodes, the
// path is:
//     evdev_code (u16) + 8 → X11 keycode
//     X11 keycode → keysym[modlevel]
//     keysym → xterm / twm

pub const MIN_KEYCODE: u8 = 8;
pub const MAX_KEYCODE: u8 = 255;

/// Number of keysyms per keycode we report in GetKeyboardMapping.
/// X11 supports up to 4 (group 1 level 0/1, group 2 level 0/1); we
/// honor the first two (unshifted, shifted).
pub const KEYSYMS_PER_KEYCODE: u8 = 2;

pub const NO_SYMBOL: u32 = 0;

// ── Keysyms (subset) ────────────────────────────────────────────────
//   Printable Latin-1 keysyms == the codepoint itself.
//   Non-printable keysyms come from keysymdef.h (high byte 0xFF).

pub const XK_BACKSPACE: u32 = 0xFF08;
pub const XK_TAB:       u32 = 0xFF09;
pub const XK_RETURN:    u32 = 0xFF0D;
pub const XK_ESCAPE:    u32 = 0xFF1B;
pub const XK_DELETE:    u32 = 0xFFFF;

pub const XK_HOME:      u32 = 0xFF50;
pub const XK_LEFT:      u32 = 0xFF51;
pub const XK_UP:        u32 = 0xFF52;
pub const XK_RIGHT:     u32 = 0xFF53;
pub const XK_DOWN:      u32 = 0xFF54;
pub const XK_END:       u32 = 0xFF57;

pub const XK_SHIFT_L:   u32 = 0xFFE1;
pub const XK_SHIFT_R:   u32 = 0xFFE2;
pub const XK_CONTROL_L: u32 = 0xFFE3;
pub const XK_CONTROL_R: u32 = 0xFFE4;
pub const XK_CAPS_LOCK: u32 = 0xFFE5;
pub const XK_ALT_L:     u32 = 0xFFE9;
pub const XK_ALT_R:     u32 = 0xFFEA;
pub const XK_SUPER_L:   u32 = 0xFFEB;

// ── Modifier state bits (KeyButMask) ───────────────────────────────

pub const MOD_SHIFT:   u16 = 1 << 0;
pub const MOD_LOCK:    u16 = 1 << 1;  // CapsLock
pub const MOD_CONTROL: u16 = 1 << 2;
pub const MOD_1:       u16 = 1 << 3;  // Alt
pub const MOD_2:       u16 = 1 << 4;
pub const MOD_3:       u16 = 1 << 5;
pub const MOD_4:       u16 = 1 << 6;  // Super
pub const MOD_5:       u16 = 1 << 7;

// ── X11 keycode → keysym table ─────────────────────────────────────
//
// Indexed by (keycode - 8) so the array is compact.  Each row has
// KEYSYMS_PER_KEYCODE entries (unshifted, shifted).

const NK: usize = (MAX_KEYCODE as usize - MIN_KEYCODE as usize) + 1;

pub static KEYSYM_TABLE: [[u32; KEYSYMS_PER_KEYCODE as usize]; NK] = build_table();

const fn build_table() -> [[u32; 2]; NK] {
    let mut t = [[NO_SYMBOL; 2]; NK];
    // evdev scancode → (lower, upper) or (unshifted, shifted) keysyms.
    // The left column is the Linux `KEY_*` scancode; the X11 keycode
    // is `scancode + 8`.
    let rows: &[(u16, u32, u32)] = &[
        (1,  XK_ESCAPE,   XK_ESCAPE),
        (2,  '1' as u32,  '!' as u32),
        (3,  '2' as u32,  '@' as u32),
        (4,  '3' as u32,  '#' as u32),
        (5,  '4' as u32,  '$' as u32),
        (6,  '5' as u32,  '%' as u32),
        (7,  '6' as u32,  '^' as u32),
        (8,  '7' as u32,  '&' as u32),
        (9,  '8' as u32,  '*' as u32),
        (10, '9' as u32,  '(' as u32),
        (11, '0' as u32,  ')' as u32),
        (12, '-' as u32,  '_' as u32),
        (13, '=' as u32,  '+' as u32),
        (14, XK_BACKSPACE, XK_BACKSPACE),
        (15, XK_TAB, XK_TAB),
        (16, 'q' as u32, 'Q' as u32),
        (17, 'w' as u32, 'W' as u32),
        (18, 'e' as u32, 'E' as u32),
        (19, 'r' as u32, 'R' as u32),
        (20, 't' as u32, 'T' as u32),
        (21, 'y' as u32, 'Y' as u32),
        (22, 'u' as u32, 'U' as u32),
        (23, 'i' as u32, 'I' as u32),
        (24, 'o' as u32, 'O' as u32),
        (25, 'p' as u32, 'P' as u32),
        (26, '[' as u32, '{' as u32),
        (27, ']' as u32, '}' as u32),
        (28, XK_RETURN, XK_RETURN),
        (29, XK_CONTROL_L, XK_CONTROL_L),
        (30, 'a' as u32, 'A' as u32),
        (31, 's' as u32, 'S' as u32),
        (32, 'd' as u32, 'D' as u32),
        (33, 'f' as u32, 'F' as u32),
        (34, 'g' as u32, 'G' as u32),
        (35, 'h' as u32, 'H' as u32),
        (36, 'j' as u32, 'J' as u32),
        (37, 'k' as u32, 'K' as u32),
        (38, 'l' as u32, 'L' as u32),
        (39, ';' as u32, ':' as u32),
        (40, '\'' as u32, '"' as u32),
        (41, '`' as u32, '~' as u32),
        (42, XK_SHIFT_L, XK_SHIFT_L),
        (43, '\\' as u32, '|' as u32),
        (44, 'z' as u32, 'Z' as u32),
        (45, 'x' as u32, 'X' as u32),
        (46, 'c' as u32, 'C' as u32),
        (47, 'v' as u32, 'V' as u32),
        (48, 'b' as u32, 'B' as u32),
        (49, 'n' as u32, 'N' as u32),
        (50, 'm' as u32, 'M' as u32),
        (51, ',' as u32, '<' as u32),
        (52, '.' as u32, '>' as u32),
        (53, '/' as u32, '?' as u32),
        (54, XK_SHIFT_R, XK_SHIFT_R),
        (56, XK_ALT_L, XK_ALT_L),
        (57, ' ' as u32, ' ' as u32),   // space
        (58, XK_CAPS_LOCK, XK_CAPS_LOCK),
        (97, XK_CONTROL_R, XK_CONTROL_R),
        (100, XK_ALT_R, XK_ALT_R),
        (102, XK_HOME, XK_HOME),
        (103, XK_UP, XK_UP),
        (105, XK_LEFT, XK_LEFT),
        (106, XK_RIGHT, XK_RIGHT),
        (107, XK_END, XK_END),
        (108, XK_DOWN, XK_DOWN),
        (111, XK_DELETE, XK_DELETE),
        (125, XK_SUPER_L, XK_SUPER_L),
    ];
    let mut i = 0;
    while i < rows.len() {
        let (scancode, lower, upper) = rows[i];
        let kc = (scancode as usize) + 8;
        if kc >= MIN_KEYCODE as usize && kc <= MAX_KEYCODE as usize {
            let idx = kc - MIN_KEYCODE as usize;
            t[idx][0] = lower;
            t[idx][1] = upper;
        }
        i += 1;
    }
    t
}

/// Return keysym[level] for the given keycode, or NO_SYMBOL if out of
/// range / unmapped.
pub fn lookup(keycode: u8, level: usize) -> u32 {
    if keycode < MIN_KEYCODE { return NO_SYMBOL; }
    let idx = (keycode - MIN_KEYCODE) as usize;
    if idx >= NK { return NO_SYMBOL; }
    if level >= KEYSYMS_PER_KEYCODE as usize { return NO_SYMBOL; }
    KEYSYM_TABLE[idx][level]
}

// ── Modifier mapping (keycodes that act as each modifier) ──────────
//
// GetModifierMapping returns an array indexed by modifier bit,
// listing the keycodes that produce that modifier.  X11 allocates 8
// slots per modifier.  Unused slots are 0.

pub const MODMAP_KEYS_PER_MOD: u8 = 2;

pub static MOD_MAPPING: [[u8; MODMAP_KEYS_PER_MOD as usize]; 8] = [
    // Shift
    [42 + 8, 54 + 8],
    // Lock (CapsLock)
    [58 + 8, 0],
    // Control
    [29 + 8, 97 + 8],
    // Mod1 (Alt)
    [56 + 8, 100 + 8],
    // Mod2
    [0, 0],
    // Mod3
    [0, 0],
    // Mod4 (Super)
    [125 + 8, 0],
    // Mod5
    [0, 0],
];
