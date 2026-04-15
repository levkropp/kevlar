// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(dead_code)]
//
// X11 Window resources.
//
// A Window holds its parent/children pointers, geometry, map state,
// attributes, event mask, and property list.  The server maintains a
// flat map of id → Window inside `ResourceMap`; the tree is expressed
// purely via parent/children XIDs.
//
// Tree layout is per-server (not per-client): client A can see client
// B's windows via QueryTree on root, just like real X11.
//
// Phase 3 implements:
//   CreateWindow(1), ChangeWindowAttributes(2), GetWindowAttributes(3),
//   DestroyWindow(4), DestroySubwindows(5), MapWindow(8),
//   MapSubwindows(9), UnmapWindow(10), ConfigureWindow(12),
//   GetGeometry(14), QueryTree(15),
//   ChangeProperty(18), DeleteProperty(19), GetProperty(20),
//   ListProperties(21)
// Events:
//   CreateNotify(16), DestroyNotify(17), UnmapNotify(18),
//   MapNotify(19), ConfigureNotify(22), PropertyNotify(28), Expose(12)

use crate::property::Property;

/// Window class (CreateWindow: `class` byte).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowClass {
    CopyFromParent = 0,
    InputOutput    = 1,
    InputOnly      = 2,
}

impl WindowClass {
    pub fn from_u16(v: u16) -> WindowClass {
        match v {
            1 => WindowClass::InputOutput,
            2 => WindowClass::InputOnly,
            _ => WindowClass::CopyFromParent,
        }
    }
}

/// X11 event-mask bits (from core protocol §3.7).
pub mod event_mask {
    pub const KEY_PRESS:            u32 = 0x0000_0001;
    pub const KEY_RELEASE:          u32 = 0x0000_0002;
    pub const BUTTON_PRESS:         u32 = 0x0000_0004;
    pub const BUTTON_RELEASE:       u32 = 0x0000_0008;
    pub const ENTER_WINDOW:         u32 = 0x0000_0010;
    pub const LEAVE_WINDOW:         u32 = 0x0000_0020;
    pub const POINTER_MOTION:       u32 = 0x0000_0040;
    pub const POINTER_MOTION_HINT:  u32 = 0x0000_0080;
    pub const BUTTON1_MOTION:       u32 = 0x0000_0100;
    pub const BUTTON2_MOTION:       u32 = 0x0000_0200;
    pub const BUTTON3_MOTION:       u32 = 0x0000_0400;
    pub const BUTTON4_MOTION:       u32 = 0x0000_0800;
    pub const BUTTON5_MOTION:       u32 = 0x0000_1000;
    pub const BUTTON_MOTION:        u32 = 0x0000_2000;
    pub const KEYMAP_STATE:         u32 = 0x0000_4000;
    pub const EXPOSURE:             u32 = 0x0000_8000;
    pub const VISIBILITY_CHANGE:    u32 = 0x0001_0000;
    pub const STRUCTURE_NOTIFY:     u32 = 0x0002_0000;
    pub const RESIZE_REDIRECT:      u32 = 0x0004_0000;
    pub const SUBSTRUCTURE_NOTIFY:  u32 = 0x0008_0000;
    pub const SUBSTRUCTURE_REDIRECT:u32 = 0x0010_0000;
    pub const FOCUS_CHANGE:         u32 = 0x0020_0000;
    pub const PROPERTY_CHANGE:      u32 = 0x0040_0000;
    pub const COLORMAP_CHANGE:      u32 = 0x0080_0000;
    pub const OWNER_GRAB_BUTTON:    u32 = 0x0100_0000;
}

/// A listener entry: (client_id, event_mask).
#[derive(Debug, Clone, Copy)]
pub struct Listener {
    pub client: u32,
    pub mask: u32,
}

#[derive(Debug)]
pub struct Window {
    // ── Identity and tree pointers ──
    pub id: u32,
    pub parent: u32,          // 0 for the root
    pub children: Vec<u32>,   // ids, front-to-back stacking
    pub owner: u32,           // client id that created it (0 = server-owned root)

    // ── Geometry ──
    pub x: i16,
    pub y: i16,
    pub width: u16,
    pub height: u16,
    pub border_width: u16,

    // ── Class/depth/visual ──
    pub class: WindowClass,
    pub depth: u8,
    pub visual: u32,

    // ── State ──
    pub mapped: bool,
    pub override_redirect: bool,

    // ── Attributes (subset — we grow as needed) ──
    pub bg_pixel: u32,
    pub border_pixel: u32,
    pub backing_store: u8,     // 0=Never, 1=WhenMapped, 2=Always
    pub save_under: bool,
    pub colormap: u32,         // 0 = CopyFromParent
    pub cursor: u32,           // 0 = None
    pub do_not_propagate: u32, // event mask that does not propagate

    /// Per-client SelectInput masks on this window.  Each entry is a
    /// (client, mask) pair.  Multiple clients may select on the same
    /// window; a wake is delivered to every client whose mask matches.
    pub listeners: Vec<Listener>,

    // ── Properties (name atom → Property) ──
    pub properties: Vec<Property>,
}

impl Window {
    pub fn new_root(id: u32, width: u16, height: u16, visual: u32) -> Self {
        Window {
            id,
            parent: 0,
            children: Vec::new(),
            owner: 0,
            x: 0,
            y: 0,
            width,
            height,
            border_width: 0,
            class: WindowClass::InputOutput,
            depth: 24,
            visual,
            mapped: true,  // root is always mapped
            override_redirect: false,
            bg_pixel: 0x00000000,
            border_pixel: 0x00FFFFFF,
            backing_store: 0,
            save_under: false,
            colormap: 0,
            cursor: 0,
            do_not_propagate: 0,
            listeners: Vec::new(),
            properties: Vec::new(),
        }
    }

    pub fn new_child(
        id: u32,
        parent: u32,
        owner: u32,
        x: i16,
        y: i16,
        width: u16,
        height: u16,
        border_width: u16,
        class: WindowClass,
        depth: u8,
        visual: u32,
    ) -> Self {
        Window {
            id,
            parent,
            children: Vec::new(),
            owner,
            x,
            y,
            width,
            height,
            border_width,
            class,
            depth,
            visual,
            mapped: false,
            override_redirect: false,
            bg_pixel: 0x00000000,
            border_pixel: 0x00000000,
            backing_store: 0,
            save_under: false,
            colormap: 0,
            cursor: 0,
            do_not_propagate: 0,
            listeners: Vec::new(),
            properties: Vec::new(),
        }
    }

    /// Combined event mask across all listeners (for quick "should we
    /// even bother generating this event?" checks).
    pub fn combined_mask(&self) -> u32 {
        self.listeners.iter().fold(0, |acc, l| acc | l.mask)
    }

    /// Add or replace a listener for `client` with `mask`.  Setting mask=0
    /// removes the listener.
    pub fn set_listener(&mut self, client: u32, mask: u32) {
        if let Some(pos) = self.listeners.iter().position(|l| l.client == client) {
            if mask == 0 {
                self.listeners.remove(pos);
            } else {
                self.listeners[pos].mask = mask;
            }
        } else if mask != 0 {
            self.listeners.push(Listener { client, mask });
        }
    }
}
