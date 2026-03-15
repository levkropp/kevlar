// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! eventfd(2) — Inter-process/thread notification counter.
//!
//! Provenance: Own (Linux eventfd(2) man page).
use core::fmt;

use crate::fs::inode::{FileLike, PollStatus};
use crate::poll::POLL_WAIT_QUEUE;
use crate::prelude::*;
use crate::user_buffer::{UserBufWriter, UserBuffer, UserBufferMut};
use kevlar_platform::spinlock::SpinLock;
use kevlar_vfs::inode::OpenOptions;
use kevlar_vfs::stat::Stat;

// ── Linux eventfd constants ────────────────────────────────────────

pub const EFD_SEMAPHORE: i32 = 1;
pub const EFD_CLOEXEC: i32 = 0o2000000;
pub const EFD_NONBLOCK: i32 = 0o4000;

// ── EventFd ────────────────────────────────────────────────────────

pub struct EventFd {
    inner: SpinLock<EventFdInner>,
}

struct EventFdInner {
    counter: u64,
    semaphore: bool,
}

impl EventFd {
    pub fn new(initval: u32, semaphore: bool) -> Arc<EventFd> {
        Arc::new(EventFd {
            inner: SpinLock::new(EventFdInner {
                counter: initval as u64,
                semaphore,
            }),
        })
    }
}

impl fmt::Debug for EventFd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventFd").finish()
    }
}

impl FileLike for EventFd {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat::zeroed())
    }

    fn read(
        &self,
        _offset: usize,
        mut buf: UserBufferMut<'_>,
        options: &OpenOptions,
    ) -> Result<usize> {
        if buf.len() < 8 {
            return Err(Errno::EINVAL.into());
        }

        // Fast path: check if counter is non-zero.
        {
            let mut inner = self.inner.lock_no_irq();
            if inner.counter > 0 {
                let val = if inner.semaphore {
                    inner.counter -= 1;
                    1u64
                } else {
                    let v = inner.counter;
                    inner.counter = 0;
                    v
                };
                drop(inner);
                POLL_WAIT_QUEUE.wake_all();
                buf.write_u64(val)?;
                return Ok(8);
            }
            if options.nonblock {
                return Err(Errno::EAGAIN.into());
            }
        }

        // Slow path: block until counter > 0.
        POLL_WAIT_QUEUE.sleep_signalable_until(|| {
            let mut inner = self.inner.lock_no_irq();
            if inner.counter > 0 {
                let val = if inner.semaphore {
                    inner.counter -= 1;
                    1u64
                } else {
                    let v = inner.counter;
                    inner.counter = 0;
                    v
                };
                drop(inner);
                POLL_WAIT_QUEUE.wake_all();
                // We need to write the value but we're in a closure.
                // Return the value, we'll write outside.
                Ok(Some(val))
            } else {
                Ok(None)
            }
        })
        .and_then(|val| {
            let mut writer = UserBufWriter::from(buf);
            writer.write_bytes(&val.to_ne_bytes())?;
            Ok(8)
        })
    }

    fn write(
        &self,
        _offset: usize,
        buf: UserBuffer<'_>,
        options: &OpenOptions,
    ) -> Result<usize> {
        if buf.len() < 8 {
            return Err(Errno::EINVAL.into());
        }

        let val = buf.read_u64()?;

        if val == u64::MAX {
            return Err(Errno::EINVAL.into());
        }

        // Add value to counter. Block if would overflow.
        {
            let mut inner = self.inner.lock_no_irq();
            if inner.counter.checked_add(val).map_or(false, |v| v < u64::MAX) {
                inner.counter += val;
                drop(inner);
                POLL_WAIT_QUEUE.wake_all();
                return Ok(8);
            }
            if options.nonblock {
                return Err(Errno::EAGAIN.into());
            }
        }

        // Slow path: block until there's room.
        let ret = POLL_WAIT_QUEUE.sleep_signalable_until(|| {
            let mut inner = self.inner.lock_no_irq();
            if inner.counter.checked_add(val).map_or(false, |v| v < u64::MAX) {
                inner.counter += val;
                Ok(Some(8usize))
            } else {
                Ok(None)
            }
        });

        POLL_WAIT_QUEUE.wake_all();
        ret
    }

    fn poll(&self) -> Result<PollStatus> {
        let inner = self.inner.lock_no_irq();
        let mut status = PollStatus::empty();
        if inner.counter > 0 {
            status |= PollStatus::POLLIN;
        }
        if inner.counter < u64::MAX - 1 {
            status |= PollStatus::POLLOUT;
        }
        Ok(status)
    }
}
