// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! /dev/input/mice — PS/2 mouse multiplexer device.
//!
//! Reads ImPS/2-format 3-byte packets from the PS/2 mouse driver's ring
//! buffer. X11 and other graphical systems read this device for pointer input.

use crate::result::{Errno, Result};
use crate::fs::inode::{FileLike, OpenOptions, PollStatus};
use crate::fs::stat::Stat;
use crate::user_buffer::{UserBufWriter, UserBufferMut, UserBuffer};
use crate::poll::POLL_WAIT_QUEUE;
use core::fmt;

pub struct MiceFile;

impl MiceFile {
    pub fn new() -> Self {
        MiceFile
    }
}

impl fmt::Debug for MiceFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MiceFile(/dev/input/mice)")
    }
}

impl FileLike for MiceFile {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            rdev: crate::fs::stat::DevId::new((13 << 8) | 63), // major=13 minor=63
            ..Stat::zeroed()
        })
    }

    fn read(&self, _offset: usize, buf: UserBufferMut<'_>, options: &OpenOptions) -> Result<usize> {
        #[cfg(target_arch = "x86_64")]
        {
            let avail = kevlar_platform::arch::ps2mouse::available();

            if avail == 0 && options.nonblock {
                return Err(Errno::EAGAIN.into());
            }

            if avail == 0 {
                // Blocking: wait until mouse data is available
                POLL_WAIT_QUEUE.sleep_signalable_until(|| {
                    if kevlar_platform::arch::ps2mouse::available() > 0 {
                        Ok(Some(()))
                    } else {
                        Ok(None)
                    }
                })?;
            }

            // Read available data
            let len = buf.len().min(kevlar_platform::arch::ps2mouse::available());
            let mut tmp = [0u8; 512];
            let n = kevlar_platform::arch::ps2mouse::read(&mut tmp[..len]);
            let mut writer = UserBufWriter::from(buf);
            writer.write_bytes(&tmp[..n])?;
            Ok(n)
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            let _ = (buf, options);
            Err(Errno::ENODEV.into())
        }
    }

    fn write(&self, _offset: usize, _buf: UserBuffer<'_>, _options: &OpenOptions) -> Result<usize> {
        Err(Errno::EINVAL.into())
    }

    fn poll(&self) -> Result<PollStatus> {
        #[cfg(target_arch = "x86_64")]
        {
            if kevlar_platform::arch::ps2mouse::available() > 0 {
                Ok(PollStatus::POLLIN)
            } else {
                Ok(PollStatus::empty())
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        Ok(PollStatus::empty())
    }
}
