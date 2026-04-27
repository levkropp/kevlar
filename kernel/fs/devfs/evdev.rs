// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! /dev/input/eventN — Linux evdev interface backed by `exts/virtio_input`.
//!
//! Each `EvdevFile` wraps one `virtio_input::InputDevice`.  Read returns
//! a stream of 24-byte `struct input_event` records (one virtio-input
//! event per record, padded with a fake timestamp).  poll()/EPOLLET
//! integrate with the rest of the kernel's wait-queue / state-gen
//! machinery so Xorg's libinput-style consumers can wait on input.
//!
//! Only the ioctls the `xf86-input-evdev` driver actually issues are
//! handled — the rest return ENOTTY, which evdev falls back from
//! gracefully (it just degrades feature availability).

use core::fmt;

use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::inode::{FileLike, OpenOptions, PollStatus};
use crate::fs::stat::Stat;
use crate::poll::POLL_WAIT_QUEUE;
use crate::result::{Errno, Result};
use crate::user_buffer::{UserBufWriter, UserBufferMut, UserBuffer};

use virtio_input::{InputDevice, VirtioInputEvent};

/// Linux `struct input_event` (24 bytes on 64-bit).
#[repr(C, packed)]
struct LinuxInputEvent {
    tv_sec: u64,
    tv_usec: u64,
    ty: u16,
    code: u16,
    value: i32,
}

const INPUT_EVENT_SIZE: usize = core::mem::size_of::<LinuxInputEvent>();

pub struct EvdevFile {
    /// Index of this device in the `virtio_input::registered_devices()` list.
    /// Resolved on every operation rather than holding an `Arc<InputDevice>`
    /// directly so the file struct stays cheap to construct from
    /// `lookup_device`.  The device list is append-only — index never
    /// becomes stale.
    index: usize,
    /// Stable device-file minor for /sys/class/input/eventN.
    minor: u32,
}

impl EvdevFile {
    pub fn new(index: usize) -> Self {
        // Linux input device minors start at 64 for /dev/input/eventN
        // (major=13).  Match that so udev-style heuristics in libinput
        // see something familiar.
        EvdevFile { index, minor: 64 + index as u32 }
    }

    fn dev(&self) -> Option<Arc<InputDevice>> {
        virtio_input::registered_devices().into_iter().nth(self.index)
    }
}

impl fmt::Debug for EvdevFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EvdevFile(/dev/input/event{})", self.index)
    }
}

impl FileLike for EvdevFile {
    fn stat(&self) -> Result<Stat> {
        use crate::fs::stat::{FileMode, S_IFCHR};
        Ok(Stat {
            mode: FileMode::new(S_IFCHR | 0o644),
            rdev: crate::fs::stat::DevId::new(((13u32) << 8) as usize | self.minor as usize),
            ..Stat::zeroed()
        })
    }

    fn read(
        &self,
        _offset: usize,
        buf: UserBufferMut<'_>,
        options: &OpenOptions,
    ) -> Result<usize> {
        let dev = match self.dev() {
            Some(d) => d,
            None => return Err(Errno::ENODEV.into()),
        };
        // evdev semantics: read returns whole `input_event`s only.
        // If the user buffer can't fit one, return EINVAL.
        if buf.len() < INPUT_EVENT_SIZE {
            return Err(Errno::EINVAL.into());
        }

        // Block (or EAGAIN) until at least one event is available.
        if !dev.has_pending() {
            if options.nonblock {
                return Err(Errno::EAGAIN.into());
            }
            POLL_WAIT_QUEUE.sleep_signalable_until(|| {
                if dev.has_pending() { Ok(Some(())) } else { Ok(None) }
            })?;
        }

        let max_events = buf.len() / INPUT_EVENT_SIZE;
        let mut raw: Vec<VirtioInputEvent> = Vec::with_capacity(max_events);
        let n = dev.drain(&mut raw, max_events);
        if n == 0 {
            // Race: another reader took our event.  Tell caller to retry.
            return Err(Errno::EAGAIN.into());
        }

        // Convert to Linux input_event with a synthesized timestamp.
        // Real Linux fills tv_sec/tv_usec from CLOCK_MONOTONIC; we use
        // a per-call snapshot of the kernel monotonic clock — close
        // enough for evdev's "is this newer than that" comparisons.
        let now_ns = crate::timer::read_monotonic_clock().nanosecs();
        let tv_sec = (now_ns / 1_000_000_000) as u64;
        let tv_usec = ((now_ns % 1_000_000_000) / 1_000) as u64;

        let mut writer = UserBufWriter::from(buf);
        for ev in &raw {
            let le = LinuxInputEvent {
                tv_sec,
                tv_usec,
                ty: ev.ty,
                code: ev.code,
                value: ev.value as i32,
            };
            // SAFETY: `LinuxInputEvent` is `repr(C, packed)` and POD —
            // any byte pattern is a valid representation.
            #[allow(unsafe_code)]
            let bytes: [u8; INPUT_EVENT_SIZE] = unsafe { core::mem::transmute(le) };
            writer.write_bytes(&bytes)?;
        }

        Ok(raw.len() * INPUT_EVENT_SIZE)
    }

    fn write(
        &self,
        _offset: usize,
        buf: UserBuffer<'_>,
        _options: &OpenOptions,
    ) -> Result<usize> {
        // Userspace can write to evdev to send LED / synth events.
        // xf86-input-evdev writes a `struct input_event[N]` block via
        // EvdevKbdCtrl every time XKB updates the LED state, and
        // CHECKS that `write() == sizeof(buffer)` — if we return 0,
        // it logs "Failed to set keyboard controls" and unloads the
        // keyboard, breaking input entirely.
        //
        // We don't currently forward LEDs to the device's statusq
        // (so the LED state in QEMU doesn't track XKB), but we DO
        // need to report success on the full write so userspace
        // sees a normal handshake.  This is a "drop the bytes
        // silently but pretend they went" pattern, same as
        // /dev/null.
        Ok(buf.len())
    }

    fn poll(&self) -> Result<PollStatus> {
        let dev = match self.dev() {
            Some(d) => d,
            None => return Err(Errno::ENODEV.into()),
        };
        let mut s = PollStatus::empty();
        if dev.has_pending() {
            s |= PollStatus::POLLIN;
        }
        Ok(s)
    }

    fn poll_gen(&self) -> u64 {
        match self.dev() {
            Some(d) => d.poll_gen(),
            None => 0,
        }
    }

    fn notify_epoll_et(&self, added: bool) {
        if let Some(d) = self.dev() {
            d.notify_epoll_et(added);
        }
    }

    fn ioctl(&self, cmd: usize, arg: usize) -> Result<isize> {
        // ioctl encoding on aarch64 (also matches x86_64):
        //   bits 0..7   = nr
        //   bits 8..15  = type
        //   bits 16..29 = size (14 bits)
        //   bits 30..31 = direction (1=write, 2=read, 3=read|write)
        const NR_MASK: usize = 0xff;
        const TYPE_MASK: usize = 0xff00;
        const SIZE_SHIFT: u32 = 16;
        const SIZE_MASK: usize = 0x3FFF << SIZE_SHIFT;
        const DIR_READ: usize = 0x80000000;
        const TYPE_E: usize = 0x4500;
        let nr = cmd & NR_MASK;
        let type_ = cmd & TYPE_MASK;
        let size = (cmd & SIZE_MASK) >> SIZE_SHIFT;
        let is_read = (cmd & DIR_READ) == DIR_READ;

        // Only handle 'E' (evdev) ioctls below.
        if type_ != TYPE_E {
            return Err(Errno::ENOTTY.into());
        }

        // EVIOCGVERSION (nr=0x01, read int)
        if nr == 0x01 && is_read && size == 4 {
            let v: u32 = 0x010001;
            let uaddr = kevlar_platform::address::UserVAddr::new_nonnull(arg)?;
            uaddr.write::<u32>(&v)?;
            return Ok(0);
        }

        // EVIOCGREP (nr=0x03, read) — keyboard auto-repeat: u32 delay
        // (ms), u32 period (ms).  Match Linux defaults.
        if nr == 0x03 && is_read {
            let rep: [u32; 2] = [250, 33];
            let uaddr = kevlar_platform::address::UserVAddr::new_nonnull(arg)?;
            uaddr.write::<[u32; 2]>(&rep)?;
            return Ok(0);
        }
        // EVIOCSREP (nr=0x03, write) — set auto-repeat.  Accept
        // silently.  xf86-input-evdev calls this in EvdevKbdCtrl()
        // every time XKB updates the repeat rate; if it returns
        // !0, evdev logs "Failed to set keyboard controls" and
        // unloads the device, breaking input entirely.
        if nr == 0x03 && !is_read {
            return Ok(0);
        }
        // EVIOCSKEYCODE (nr=0x04, write) — set scancode mapping.
        // Accept silently.
        if nr == 0x04 {
            return Ok(0);
        }

        // EVIOCGID (nr=0x02, read struct input_id, 8 bytes)
        if nr == 0x02 && is_read && size == 8 {
            #[repr(C, packed)]
            struct InputId { bustype: u16, vendor: u16, product: u16, version: u16 }
            let id = InputId {
                bustype: 0x06, // BUS_VIRTUAL
                vendor: 0x1AF4,
                product: 0x0001 + self.index as u16,
                version: 0x0001,
            };
            let uaddr = kevlar_platform::address::UserVAddr::new_nonnull(arg)?;
            uaddr.write::<InputId>(&id)?;
            return Ok(0);
        }

        // EVIOCGNAME(len) (nr=0x06, read N bytes)
        if nr == 0x06 && is_read {
            let dev = match self.dev() {
                Some(d) => d,
                None => return Err(Errno::ENODEV.into()),
            };
            let name = dev.name.lock().clone();
            let mut bytes = name.into_bytes();
            bytes.push(0);
            let n = core::cmp::min(bytes.len(), size);
            let uaddr = kevlar_platform::address::UserVAddr::new_nonnull(arg)?;
            uaddr.write_bytes(&bytes[..n])?;
            return Ok(n as isize);
        }

        // EVIOCGPHYS(len) (nr=0x07) and EVIOCGUNIQ (nr=0x08) — return
        // empty strings.  Some evdev clients require these to succeed.
        if (nr == 0x07 || nr == 0x08) && is_read {
            let n = core::cmp::min(1, size);
            if n > 0 {
                let uaddr = kevlar_platform::address::UserVAddr::new_nonnull(arg)?;
                uaddr.write_bytes(&[0])?;
            }
            return Ok(n as isize);
        }

        // EVIOCGPROP(len) (nr=0x09) — input device properties bitmap.
        // Return all-zeros, matching "no special properties".
        if nr == 0x09 && is_read {
            let uaddr = kevlar_platform::address::UserVAddr::new_nonnull(arg)?;
            let zeros = alloc::vec![0u8; size];
            uaddr.write_bytes(&zeros)?;
            return Ok(size as isize);
        }

        // EVIOCGKEY/LED/SND/SW (nr=0x18..0x1b) — current state bitmaps.
        // Return all-zeros (no keys held, no LEDs lit, etc.).
        if (0x18..=0x1b).contains(&nr) && is_read {
            let uaddr = kevlar_platform::address::UserVAddr::new_nonnull(arg)?;
            let zeros = alloc::vec![0u8; size];
            uaddr.write_bytes(&zeros)?;
            return Ok(size as isize);
        }

        // EVIOCGBIT(ev_type, len) — supported event-code bitmap.
        // Encoded as nr = 0x20 + ev_type.  Read the bitmap directly
        // from `InputDevice::ev_bits`, which the driver populated at
        // probe time from virtio-input config space.  Honest answers
        // here let xf86-input-evdev distinguish keyboard from mouse;
        // earlier "set every key bit" stubs caused Xorg to configure
        // every device as both keyboard AND tablet.
        if (0x20..=0x3f).contains(&nr) && is_read {
            let ev_type = (nr - 0x20) as usize;
            let dev = match self.dev() {
                Some(d) => d,
                None => return Err(Errno::ENODEV.into()),
            };
            let mut bits = alloc::vec![0u8; size];
            if ev_type == 0 {
                // EVIOCGBIT(0) reports the set of event TYPES this
                // device supports.  Build a bitmap by checking which
                // per-type bitmaps are non-empty in ev_bits.
                let set_bit = |bits: &mut alloc::vec::Vec<u8>, n: usize| {
                    let byte = n / 8;
                    let bit = n % 8;
                    if byte < bits.len() {
                        bits[byte] |= 1 << bit;
                    }
                };
                // EV_SYN is implicit on every input device.
                set_bit(&mut bits, 0);
                for ty in 1..32 {
                    if !dev.ev_bits[ty].lock().is_empty() {
                        set_bit(&mut bits, ty);
                    }
                }
            } else if ev_type < 32 {
                // Copy the device-reported bitmap up to the user's
                // requested size.  Pad with zeros if the user asked
                // for more than the device exposes.
                let stored = dev.ev_bits[ev_type].lock();
                let n = core::cmp::min(stored.len(), size);
                bits[..n].copy_from_slice(&stored[..n]);
            }
            let uaddr = kevlar_platform::address::UserVAddr::new_nonnull(arg)?;
            uaddr.write_bytes(&bits)?;
            return Ok(size as isize);
        }

        // EVIOCGABS(axis) (nr=0x40 + axis, read struct input_absinfo)
        // input_absinfo is 24 bytes: 6 × i32 (value, min, max, fuzz, flat, resolution).
        if (0x40..=0x7f).contains(&nr) && is_read {
            let axis = nr - 0x40;
            #[repr(C, packed)]
            struct AbsInfo { value: i32, min: i32, max: i32, fuzz: i32, flat: i32, resolution: i32 }
            // For ABS_X (0) and ABS_Y (1) on virtio-tablet, range is
            // 0..32767 (the standard virtio-input absmax).
            let info = if axis < 2 {
                AbsInfo { value: 0, min: 0, max: 32767, fuzz: 0, flat: 0, resolution: 0 }
            } else {
                AbsInfo { value: 0, min: 0, max: 0, fuzz: 0, flat: 0, resolution: 0 }
            };
            let uaddr = kevlar_platform::address::UserVAddr::new_nonnull(arg)?;
            uaddr.write::<AbsInfo>(&info)?;
            return Ok(0);
        }

        // EVIOCGRAB (nr=0x90, write int) — exclusive grab.  Accept silently.
        if nr == 0x90 {
            return Ok(0);
        }

        // Unknown evdev ioctl — evdev clients tolerate ENOTTY.
        Err(Errno::ENOTTY.into())
    }
}
