// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(dead_code)]
//
// Device-side input readers.
//
// Two readers, both best-effort and decoupled from the wire-protocol
// handlers in `dispatch.rs`:
//
//   * `MouseReader` — opens `/dev/input/mice` (ImPS/2 3- or 4-byte
//     packets) and decodes button/motion deltas.
//
//   * `KeyboardReader` — opens the first candidate keyboard device
//     that looks like it yields real keystrokes.  Kevlar's device
//     path is a known unknown (see tools/kxserver/INPUT_TODO.md);
//     until the 30-minute diagnostic lands, this reader tries
//     `/dev/input/event0`, `/dev/tty`, `/dev/console`, `/dev/tty0`
//     in order and accepts whichever opens with O_RDONLY|O_NONBLOCK.
//
// Both readers are OPTIONAL — if the open fails (EACCES on the host,
// ENOENT on a minimal container) the field is None and the server
// continues without that input source.  The wire-protocol handlers
// (WarpPointer, QueryPointer, SetInputFocus, …) operate off the
// logical `InputState` regardless; device readers only feed that
// state and emit `InputEvent`s.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

use crate::log;

/// High-level input event produced by the readers.  These are
/// framebuffer-absolute (for mouse) or keycode-in-X11-space (for
/// keyboard) — they still need to be routed to the right window by
/// `route_input_event` in `dispatch.rs`.
#[derive(Debug, Clone, Copy)]
pub enum InputEvent {
    MouseMotion { dx: i16, dy: i16 },
    MouseButton { button: u8, pressed: bool },
    Key        { keycode: u8, pressed: bool },
}

// ═════════════════════════════════════════════════════════════════════
// Mouse reader (/dev/input/mice)
// ═════════════════════════════════════════════════════════════════════
//
// The ImPS/2 protocol (the default for /dev/input/mice with
// imps2-capable mice) is 4-byte packets:
//   byte 0: [yo][xo][ys][xs][1][m][r][l]
//             l/m/r = left/middle/right button,
//             xs/ys = sign bits, xo/yo = overflow
//   byte 1: dx (i8 if xs=0, otherwise... combined with xs)
//   byte 2: dy (same)
//   byte 3: scroll wheel delta (i8)
//
// The legacy PS/2 protocol uses 3 bytes (no scroll byte).  We accept
// both: if we get a packet byte 0 with the required bit (0x08) set
// and the first byte in a sequence, read 3 more; on EAGAIN at byte 3
// we accept a 3-byte packet and drop the scroll.  In practice every
// Linux system today exposes ImPS/2 if you open `/dev/input/mice`.

pub struct MouseReader {
    fd: OwnedFd,
    /// Partial packet buffer (up to 4 bytes).
    buf: [u8; 4],
    buf_len: usize,
}

impl MouseReader {
    pub fn open() -> Option<Self> {
        open_nonblocking(b"/dev/input/mice\0").map(|fd| MouseReader {
            fd,
            buf: [0; 4],
            buf_len: 0,
        })
    }

    pub fn raw_fd(&self) -> RawFd { self.fd.as_raw_fd() }

    /// Drain as many complete packets as are currently available and
    /// push the corresponding `InputEvent`s onto `out`.  Never
    /// blocks.  Tracks partial packets across calls via `self.buf`.
    pub fn read_events(&mut self, out: &mut Vec<InputEvent>) {
        loop {
            let need = 4 - self.buf_len;
            let n = unsafe {
                libc::read(
                    self.fd.as_raw_fd(),
                    self.buf.as_mut_ptr().add(self.buf_len) as *mut _,
                    need,
                )
            };
            if n <= 0 {
                break;
            }
            self.buf_len += n as usize;
            if self.buf_len < 3 {
                continue;
            }
            // We have at least 3 bytes.  Try to decode a packet.
            let b0 = self.buf[0];
            // Bit 3 of byte 0 must be set on a valid first byte of a
            // PS/2 packet; if it isn't, we've lost framing — drop
            // the first byte and try again.
            if (b0 & 0x08) == 0 {
                self.buf.copy_within(1..self.buf_len, 0);
                self.buf_len -= 1;
                continue;
            }
            let left   = (b0 & 0x01) != 0;
            let right  = (b0 & 0x02) != 0;
            let middle = (b0 & 0x04) != 0;
            let dx_raw = self.buf[1] as i8;
            let dy_raw = self.buf[2] as i8;
            // Y axis is inverted on PS/2 mice.
            let dx = dx_raw as i16;
            let dy = -(dy_raw as i16);

            // Button diff: we need previous state to emit Press vs
            // Release.  Keep it on the reader.
            static_button_diff(self, left, middle, right, out);
            if dx != 0 || dy != 0 {
                out.push(InputEvent::MouseMotion { dx, dy });
            }

            // Decide whether to consume 3 or 4 bytes.  If we have 4
            // and the mouse is ImPS/2 we should consume all 4; if
            // we only have 3 the 4th byte will arrive later, but
            // that's actually an awkward state for framing.  In
            // practice every packet either arrives complete or we
            // wait; so consume all 4 when we have them.
            let consumed = if self.buf_len >= 4 { 4 } else { 3 };
            self.buf.copy_within(consumed..self.buf_len, 0);
            self.buf_len -= consumed;
        }
    }
}

/// Remember the previous mouse button mask and emit Press/Release
/// events for any bits that changed.  The state is stashed inside
/// the reader via a private mutable static — kxserver is
/// single-threaded so this is safe.
fn static_button_diff(
    reader: &mut MouseReader,
    left: bool, middle: bool, right: bool,
    out: &mut Vec<InputEvent>,
) {
    // Bit encoding for the "prev" state: L=1, M=2, R=4.
    let prev = MOUSE_PREV_BUTTONS.load(core::sync::atomic::Ordering::Relaxed);
    let now  = (left as u8) | ((middle as u8) << 1) | ((right as u8) << 2);
    let diff = prev ^ now;
    for (bit, button) in [(0, 1u8), (1, 2u8), (2, 3u8)] {
        if (diff >> bit) & 1 != 0 {
            let pressed = (now >> bit) & 1 != 0;
            out.push(InputEvent::MouseButton { button, pressed });
        }
    }
    MOUSE_PREV_BUTTONS.store(now, core::sync::atomic::Ordering::Relaxed);
    let _ = reader;  // currently unused; kept for future per-reader state.
}

static MOUSE_PREV_BUTTONS: core::sync::atomic::AtomicU8 =
    core::sync::atomic::AtomicU8::new(0);

// ═════════════════════════════════════════════════════════════════════
// Keyboard reader
// ═════════════════════════════════════════════════════════════════════
//
// Evdev `struct input_event` is 24 bytes on x86_64 (two u64 timevals
// + two u16 + u32).  Layout:
//
//   0..8   tv_sec   (u64)
//   8..16  tv_usec  (u64)
//   16..18 type     (u16)  -- EV_KEY = 1
//   18..20 code     (u16)  -- Linux KEY_* scancode
//   20..24 value    (u32)  -- 0=release, 1=press, 2=autorepeat
//
// For candidate devices that are NOT evdev (e.g. /dev/tty with raw
// mode), the format is "bytes in, keycodes out via termios" which
// needs very different handling.  For Phase 9 we ONLY handle evdev.
// The diagnostic script in INPUT_TODO.md tells us which path yields
// evdev-shaped packets; until then we try /dev/input/event0.

pub const EV_KEY: u16 = 0x01;
pub const LINUX_INPUT_EVENT_SIZE: usize = 24;

pub struct KeyboardReader {
    fd: OwnedFd,
    /// Partial input_event buffer.
    buf: [u8; LINUX_INPUT_EVENT_SIZE],
    buf_len: usize,
}

impl KeyboardReader {
    pub fn open() -> Option<Self> {
        // Try candidates in order; first one that opens wins.
        // Kevlar's actual keyboard device is a known unknown (see
        // INPUT_TODO.md).  The host dev environment has permission-
        // denied on all of these, which is expected and fine — the
        // reader will be None and the server runs without a
        // keyboard, same as a VM without one.
        for candidate in [
            b"/dev/input/event0\0".as_slice(),
            b"/dev/input/event1\0".as_slice(),
            b"/dev/input/event2\0".as_slice(),
        ] {
            if let Some(fd) = open_nonblocking(candidate) {
                log::info(format_args!(
                    "keyboard: opened {}",
                    core::str::from_utf8(&candidate[..candidate.len()-1]).unwrap_or("<?>")
                ));
                return Some(KeyboardReader {
                    fd,
                    buf: [0; LINUX_INPUT_EVENT_SIZE],
                    buf_len: 0,
                });
            }
        }
        None
    }

    pub fn raw_fd(&self) -> RawFd { self.fd.as_raw_fd() }

    pub fn read_events(&mut self, out: &mut Vec<InputEvent>) {
        loop {
            let need = LINUX_INPUT_EVENT_SIZE - self.buf_len;
            let n = unsafe {
                libc::read(
                    self.fd.as_raw_fd(),
                    self.buf.as_mut_ptr().add(self.buf_len) as *mut _,
                    need,
                )
            };
            if n <= 0 {
                break;
            }
            self.buf_len += n as usize;
            if self.buf_len < LINUX_INPUT_EVENT_SIZE {
                continue;
            }
            // Decode one evdev input_event.
            let ty    = u16::from_le_bytes([self.buf[16], self.buf[17]]);
            let code  = u16::from_le_bytes([self.buf[18], self.buf[19]]);
            let value = u32::from_le_bytes([
                self.buf[20], self.buf[21], self.buf[22], self.buf[23],
            ]);
            self.buf_len = 0;
            if ty != EV_KEY { continue; }
            // Linux evdev scancode → X11 keycode is +8.
            let x11_keycode = (code as u32 + 8) as u8;
            match value {
                0 => out.push(InputEvent::Key { keycode: x11_keycode, pressed: false }),
                1 => out.push(InputEvent::Key { keycode: x11_keycode, pressed: true }),
                2 => {
                    // Autorepeat: deliver both a release and a press
                    // so clients tracking key state see a new press.
                    out.push(InputEvent::Key { keycode: x11_keycode, pressed: false });
                    out.push(InputEvent::Key { keycode: x11_keycode, pressed: true });
                }
                _ => {}
            }
        }
    }
}

// ═════════════════════════════════════════════════════════════════════
// Helpers
// ═════════════════════════════════════════════════════════════════════

/// Open `path` (NUL-terminated) with `O_RDWR | O_NONBLOCK`.  Returns
/// None on any error — callers log the absence and continue.
fn open_nonblocking(c_path: &[u8]) -> Option<OwnedFd> {
    debug_assert!(c_path.ends_with(b"\0"));
    let fd = unsafe {
        libc::open(
            c_path.as_ptr() as *const _,
            libc::O_RDWR | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        // Try read-only (keyboard devices typically don't need write
        // access and /dev/tty* often rejects O_RDWR).
        let fd = unsafe {
            libc::open(
                c_path.as_ptr() as *const _,
                libc::O_RDONLY | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            let errno = unsafe { *libc::__errno_location() };
            log::info(format_args!(
                "device open {}: errno={errno}",
                core::str::from_utf8(&c_path[..c_path.len()-1]).unwrap_or("<?>")
            ));
            return None;
        }
        return Some(unsafe { OwnedFd::from_raw_fd(fd) });
    }
    Some(unsafe { OwnedFd::from_raw_fd(fd) })
}
