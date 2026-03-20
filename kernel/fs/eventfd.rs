// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! eventfd(2) — Inter-process/thread notification counter.
//!
//! Fully lock-free: the counter is an AtomicU64, updated via CAS.
//! No spinlock needed — the semaphore flag is immutable after construction.
//!
//! Provenance: Own (Linux eventfd(2) man page).
use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::fs::inode::{FileLike, PollStatus};
use crate::poll::POLL_WAIT_QUEUE;
use crate::prelude::*;
use crate::user_buffer::{UserBufWriter, UserBuffer, UserBufferMut};
use kevlar_vfs::inode::OpenOptions;
use kevlar_vfs::stat::Stat;

// ── Linux eventfd constants ────────────────────────────────────────

pub const EFD_SEMAPHORE: i32 = 1;
pub const EFD_CLOEXEC: i32 = 0o2000000;
pub const EFD_NONBLOCK: i32 = 0o4000;

// ── EventFd ────────────────────────────────────────────────────────

pub struct EventFd {
    /// The counter — sole source of truth, updated via CAS.
    counter: AtomicU64,
    /// Generation counter for epoll poll-result caching.
    state_gen: AtomicU64,
    /// EFD_SEMAPHORE mode: read returns 1 and decrements, not drain.
    semaphore: bool,
}

impl EventFd {
    pub fn new(initval: u32, semaphore: bool) -> Arc<EventFd> {
        Arc::new(EventFd {
            counter: AtomicU64::new(initval as u64),
            state_gen: AtomicU64::new(1),
            semaphore,
        })
    }

    /// Try to consume the counter (CAS loop). Returns the value read,
    /// or None if counter is zero.
    #[inline(always)]
    fn try_read_counter(&self) -> Option<u64> {
        loop {
            let cur = self.counter.load(Ordering::Acquire);
            if cur == 0 {
                return None;
            }
            let (new, val) = if self.semaphore {
                (cur - 1, 1u64)
            } else {
                (0, cur)
            };
            if self.counter.compare_exchange_weak(cur, new, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                self.state_gen.fetch_add(1, Ordering::Relaxed);
                return Some(val);
            }
        }
    }

    /// Try to add val to the counter (CAS loop). Returns true on success,
    /// false if it would overflow.
    #[inline(always)]
    fn try_write_counter(&self, val: u64) -> bool {
        loop {
            let cur = self.counter.load(Ordering::Acquire);
            let new = match cur.checked_add(val) {
                Some(v) if v < u64::MAX => v,
                _ => return false,
            };
            if self.counter.compare_exchange_weak(cur, new, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                self.state_gen.fetch_add(1, Ordering::Relaxed);
                return true;
            }
        }
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

        // Fast path: CAS read — no lock needed.
        if let Some(val) = self.try_read_counter() {
            POLL_WAIT_QUEUE.wake_all();
            buf.write_u64(val)?;
            return Ok(8);
        }

        if options.nonblock {
            return Err(Errno::EAGAIN.into());
        }

        // Slow path: block until counter > 0.
        POLL_WAIT_QUEUE.sleep_signalable_until(|| {
            if let Some(val) = self.try_read_counter() {
                POLL_WAIT_QUEUE.wake_all();
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

        // Fast path: CAS write — no lock needed.
        if self.try_write_counter(val) {
            POLL_WAIT_QUEUE.wake_all();
            return Ok(8);
        }

        if options.nonblock {
            return Err(Errno::EAGAIN.into());
        }

        // Slow path: block until there's room.
        let ret = POLL_WAIT_QUEUE.sleep_signalable_until(|| {
            if self.try_write_counter(val) {
                Ok(Some(8usize))
            } else {
                Ok(None)
            }
        });

        POLL_WAIT_QUEUE.wake_all();
        ret
    }

    fn poll(&self) -> Result<PollStatus> {
        let counter = self.counter.load(Ordering::Relaxed);
        let mut status = PollStatus::empty();
        if counter > 0 {
            status |= PollStatus::POLLIN;
        }
        if counter < u64::MAX - 1 {
            status |= PollStatus::POLLOUT;
        }
        Ok(status)
    }

    fn poll_gen(&self) -> u64 {
        self.state_gen.load(Ordering::Relaxed)
    }

    fn poll_gen_atomic(&self) -> Option<&core::sync::atomic::AtomicU64> {
        Some(&self.state_gen)
    }
}
