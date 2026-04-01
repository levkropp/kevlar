// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! PS/2 keyboard driver for the i8042 controller.
//!
//! Reads scan codes from port 0x60 on IRQ1 and translates them to ASCII
//! characters via a simple Set 1 scan code table. Characters are fed into
//! the console input handler (same path as serial input).

use x86::io::inb;
use crate::handler;

// ─── Scan Code Set 1 → ASCII translation ────────────────────────────────────

// Map from Set 1 make codes (0x00-0x58) to ASCII. 0 = no mapping.
static SCANCODE_ASCII: [u8; 89] = [
    0,   27,  b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', // 0x00-0x09
    b'9', b'0', b'-', b'=', 8,   b'\t',                        // 0x0A-0x0F
    b'q', b'w', b'e', b'r', b't', b'y', b'u', b'i', b'o', b'p', // 0x10-0x19
    b'[', b']', b'\n', 0,                                       // 0x1A-0x1D (1D=LCtrl)
    b'a', b's', b'd', b'f', b'g', b'h', b'j', b'k', b'l',     // 0x1E-0x26
    b';', b'\'', b'`', 0,                                       // 0x27-0x2A (2A=LShift)
    b'\\', b'z', b'x', b'c', b'v', b'b', b'n', b'm',          // 0x2B-0x32
    b',', b'.', b'/', 0,                                         // 0x33-0x36 (36=RShift)
    b'*', 0,   b' ', 0,                                         // 0x37-0x3A (38=LAlt, 3A=CapsLock)
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0,                               // 0x3B-0x44 (F1-F10)
    0, 0,                                                         // 0x45-0x46 (NumLock, ScrollLock)
    b'7', b'8', b'9', b'-',                                     // 0x47-0x4A (Keypad)
    b'4', b'5', b'6', b'+',                                     // 0x4B-0x4E
    b'1', b'2', b'3',                                           // 0x4F-0x51
    b'0', b'.',                                                  // 0x52-0x53
    0, 0, 0,                                                     // 0x54-0x56
    0, 0,                                                        // 0x57-0x58 (F11, F12)
];

static SCANCODE_SHIFT: [u8; 89] = [
    0,   27,  b'!', b'@', b'#', b'$', b'%', b'^', b'&', b'*', // 0x00-0x09
    b'(', b')', b'_', b'+', 8,   b'\t',                        // 0x0A-0x0F
    b'Q', b'W', b'E', b'R', b'T', b'Y', b'U', b'I', b'O', b'P', // 0x10-0x19
    b'{', b'}', b'\n', 0,                                       // 0x1A-0x1D
    b'A', b'S', b'D', b'F', b'G', b'H', b'J', b'K', b'L',     // 0x1E-0x26
    b':', b'"', b'~', 0,                                         // 0x27-0x2A
    b'|', b'Z', b'X', b'C', b'V', b'B', b'N', b'M',           // 0x2B-0x32
    b'<', b'>', b'?', 0,                                         // 0x33-0x36
    b'*', 0,   b' ', 0,                                         // 0x37-0x3A
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0,                               // 0x3B-0x44
    0, 0,                                                         // 0x45-0x46
    b'7', b'8', b'9', b'-',                                     // 0x47-0x4A
    b'4', b'5', b'6', b'+',                                     // 0x4B-0x4E
    b'1', b'2', b'3',                                           // 0x4F-0x51
    b'0', b'.',                                                  // 0x52-0x53
    0, 0, 0,                                                     // 0x54-0x56
    0, 0,                                                        // 0x57-0x58
];

// Modifier key scan codes (Set 1 make codes)
const SC_LSHIFT_MAKE: u8 = 0x2A;
const SC_RSHIFT_MAKE: u8 = 0x36;
const SC_LCTRL_MAKE: u8 = 0x1D;
const SC_CAPSLOCK_MAKE: u8 = 0x3A;

// ─── State ──────────────────────────────────────────────────────────────────

use core::sync::atomic::{AtomicBool, Ordering};

static SHIFT_HELD: AtomicBool = AtomicBool::new(false);
static CTRL_HELD: AtomicBool = AtomicBool::new(false);
static CAPS_LOCK: AtomicBool = AtomicBool::new(false);

// ─── IRQ Handler ────────────────────────────────────────────────────────────

/// Called from the IRQ1 interrupt handler. Reads the scan code from the
/// i8042 data port and translates it to a character for the console.
#[allow(unsafe_code)]
pub fn ps2_keyboard_irq_handler() {
    let scancode = unsafe { inb(0x60) };

    // Break codes (key release) have bit 7 set
    let is_break = scancode & 0x80 != 0;
    let make_code = scancode & 0x7F;

    // Handle modifier keys
    match make_code {
        SC_LSHIFT_MAKE | SC_RSHIFT_MAKE => {
            SHIFT_HELD.store(!is_break, Ordering::Relaxed);
            return;
        }
        SC_LCTRL_MAKE => {
            CTRL_HELD.store(!is_break, Ordering::Relaxed);
            return;
        }
        SC_CAPSLOCK_MAKE if !is_break => {
            let current = CAPS_LOCK.load(Ordering::Relaxed);
            CAPS_LOCK.store(!current, Ordering::Relaxed);
            return;
        }
        _ => {}
    }

    // Only process make (press) events
    if is_break {
        return;
    }

    // Extended scan codes (0xE0 prefix) — arrow keys, etc.
    // For now, skip them.
    if scancode == 0xE0 {
        return;
    }

    if (make_code as usize) >= SCANCODE_ASCII.len() {
        return;
    }

    let shift = SHIFT_HELD.load(Ordering::Relaxed);
    let ctrl = CTRL_HELD.load(Ordering::Relaxed);
    let caps = CAPS_LOCK.load(Ordering::Relaxed);

    let mut ch = if shift {
        SCANCODE_SHIFT[make_code as usize]
    } else {
        SCANCODE_ASCII[make_code as usize]
    };

    // CapsLock toggles case for letters
    if caps && ch >= b'a' && ch <= b'z' {
        ch -= 32; // to uppercase
    } else if caps && ch >= b'A' && ch <= b'Z' {
        ch += 32; // to lowercase (shift+caps = lowercase)
    }

    if ch == 0 {
        return;
    }

    // Ctrl+letter → control character (0x01-0x1A)
    if ctrl {
        if ch >= b'a' && ch <= b'z' {
            ch = ch - b'a' + 1;
        } else if ch >= b'A' && ch <= b'Z' {
            ch = ch - b'A' + 1;
        }
    }

    handler().handle_console_rx(ch);
}

/// Initialize the PS/2 keyboard controller.
/// Drains any pending data from the i8042 buffer.
#[allow(unsafe_code)]
pub fn init() {
    // Drain any pending bytes from the keyboard buffer
    for _ in 0..16 {
        let status = unsafe { inb(0x64) };
        if status & 1 == 0 {
            break;
        }
        let _ = unsafe { inb(0x60) };
    }
}
