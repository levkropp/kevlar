// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(dead_code)]
//
// Input state (pointer + keyboard focus + modifiers).
//
// Phase 7 splits input handling into three layers:
//
// 1. A pure `InputState` struct held on `ServerState`.  This is what
//    the wire handlers (QueryPointer, GetInputFocus, QueryKeymap,
//    …) read and write.  No device I/O, no allocation, no globals.
//
// 2. Event emission helpers in `event.rs` that build 32-byte wire
//    blocks for KeyPress, KeyRelease, ButtonPress, ButtonRelease,
//    MotionNotify, EnterNotify, LeaveNotify, FocusIn, FocusOut.
//
// 3. (Kevlar-only, not in host builds) a device-reader that opens
//    `/dev/input/mice` and whichever candidate keyboard device the
//    Phase-7 diagnostic finds.  Those readers call into (1) and (2)
//    to update state and deliver events.  The device discovery step
//    is documented in the plan as a 30-minute investigation that
//    HAS NOT BEEN DONE YET — it requires running on Kevlar, which
//    happens outside this host smoke-test harness.
//
// Today, (1) and (2) are built and tested on the dev host via a
// purely wire-protocol-driven smoke test.  (3) is a follow-up ticket
// that lives in `tools/kxserver/INPUT_TODO.md`.

use crate::setup;

/// Snapshot of the logical pointer + keyboard state the server
/// exposes to clients.  Coordinates are in screen-absolute pixels.
#[derive(Debug, Clone)]
pub struct InputState {
    pub pointer_x: i16,
    pub pointer_y: i16,
    /// Button and modifier state, in the X11 "KeyButMask" layout:
    ///   bit 0..7:  modifiers (Shift, Lock, Control, Mod1..Mod5)
    ///   bit 8..12: buttons 1..5
    pub mask: u16,
    /// Current focus window id.  0 = None, setup::ROOT_WINDOW_ID = Root,
    /// anything else = the XID of a client-owned window.
    pub focus_window: u32,
    /// RevertTo behavior saved from the last SetInputFocus.
    ///   0 = None, 1 = PointerRoot, 2 = Parent
    pub focus_revert_to: u8,
}

impl InputState {
    pub fn new() -> Self {
        InputState {
            pointer_x: (setup::SCREEN_WIDTH / 2) as i16,
            pointer_y: (setup::SCREEN_HEIGHT / 2) as i16,
            mask: 0,
            focus_window: setup::ROOT_WINDOW_ID,
            focus_revert_to: 2,  // Parent
        }
    }

    pub fn set_pointer(&mut self, x: i16, y: i16) {
        self.pointer_x = x.clamp(0, (setup::SCREEN_WIDTH  as i16) - 1);
        self.pointer_y = y.clamp(0, (setup::SCREEN_HEIGHT as i16) - 1);
    }

    pub fn button_down(&mut self, button: u8) {
        if (1..=5).contains(&button) {
            self.mask |= 1 << (7 + button as u16);
        }
    }

    pub fn button_up(&mut self, button: u8) {
        if (1..=5).contains(&button) {
            self.mask &= !(1 << (7 + button as u16));
        }
    }

    pub fn set_modifier(&mut self, mod_bit: u16, down: bool) {
        if down { self.mask |= mod_bit; } else { self.mask &= !mod_bit; }
    }

    pub fn set_focus(&mut self, window: u32, revert_to: u8) {
        self.focus_window = window;
        self.focus_revert_to = revert_to;
    }
}
