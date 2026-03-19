// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! timerfd(2) — Timer file descriptor for epoll-driven timeouts.
//!
//! Provenance: Own (Linux timerfd_create(2), timerfd_settime(2) man pages).
//!
//! Expiration is checked lazily in poll()/read() by comparing the current
//! monotonic clock against the armed deadline.  The timer IRQ periodically
//! wakes POLL_WAIT_QUEUE so that sleeping epoll callers re-check.
use core::fmt;

use crate::fs::inode::{FileLike, PollStatus};
use crate::poll::POLL_WAIT_QUEUE;
use crate::prelude::*;
use crate::timer;
use crate::user_buffer::{UserBufWriter, UserBufferMut};
use kevlar_platform::spinlock::SpinLock;
use kevlar_vfs::inode::OpenOptions;
use kevlar_vfs::stat::Stat;

// ── Linux timerfd constants ────────────────────────────────────────

pub const TFD_CLOEXEC: i32 = 0o2000000;
pub const TFD_NONBLOCK: i32 = 0o4000;

pub const CLOCK_REALTIME: i32 = 0;
pub const CLOCK_MONOTONIC: i32 = 1;

// ── TimerFd ────────────────────────────────────────────────────────

pub struct TimerFd {
    inner: SpinLock<TimerFdInner>,
}

struct TimerFdInner {
    /// Absolute nanoseconds for next expiration (0 = disarmed).
    next_fire_ns: u64,
    /// Interval for periodic timers in nanoseconds (0 = one-shot).
    interval_ns: u64,
    /// Number of expirations since last read.
    expirations: u64,
}

impl TimerFd {
    pub fn new() -> Arc<TimerFd> {
        Arc::new(TimerFd {
            inner: SpinLock::new(TimerFdInner {
                next_fire_ns: 0,
                interval_ns: 0,
                expirations: 0,
            }),
        })
    }

    /// Arms or disarms the timer. Called from timerfd_settime syscall.
    ///
    /// `value_sec`/`value_nsec`: initial expiration (0,0 = disarm).
    /// `interval_sec`/`interval_nsec`: repeat interval (0,0 = one-shot).
    pub fn settime(
        &self,
        value_sec: i64,
        value_nsec: i64,
        interval_sec: i64,
        interval_nsec: i64,
    ) {
        let mut inner = self.inner.lock();

        if value_sec == 0 && value_nsec == 0 {
            // Disarm.
            inner.next_fire_ns = 0;
            inner.interval_ns = 0;
            inner.expirations = 0;
            return;
        }

        let now_ns = timer::read_monotonic_clock().nanosecs() as u64;
        let delay_ns = (value_sec as u64).saturating_mul(1_000_000_000).saturating_add(value_nsec as u64);
        inner.next_fire_ns = now_ns.saturating_add(delay_ns);
        inner.interval_ns = (interval_sec as u64).saturating_mul(1_000_000_000).saturating_add(interval_nsec as u64);
        inner.expirations = 0;
    }

    /// Returns (remaining_ns, interval_ns) for timerfd_gettime.
    pub fn gettime(&self) -> (u64, u64) {
        let mut inner = self.inner.lock();
        Self::check_expiry(&mut inner);
        let remaining = if inner.next_fire_ns == 0 {
            0
        } else {
            let now = timer::read_monotonic_clock().nanosecs() as u64;
            inner.next_fire_ns.saturating_sub(now)
        };
        (remaining, inner.interval_ns)
    }

    /// Check if the timer has expired and update expirations count.
    fn check_expiry(inner: &mut TimerFdInner) {
        if inner.next_fire_ns == 0 {
            return; // Disarmed.
        }

        let now_ns = timer::read_monotonic_clock().nanosecs() as u64;
        if now_ns < inner.next_fire_ns {
            return; // Not yet.
        }

        if inner.interval_ns > 0 {
            // Periodic: count how many intervals have elapsed.
            let elapsed = now_ns - inner.next_fire_ns;
            let extra = elapsed / inner.interval_ns;
            inner.expirations += 1 + extra;
            inner.next_fire_ns += (1 + extra) * inner.interval_ns;
        } else {
            // One-shot: fire once, disarm.
            inner.expirations += 1;
            inner.next_fire_ns = 0;
        }
    }
}

impl fmt::Debug for TimerFd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TimerFd").finish()
    }
}

impl FileLike for TimerFd {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat::zeroed())
    }

    fn read(
        &self,
        _offset: usize,
        buf: UserBufferMut<'_>,
        options: &OpenOptions,
    ) -> Result<usize> {
        if buf.len() < 8 {
            return Err(Errno::EINVAL.into());
        }

        // Fast path.
        {
            let mut inner = self.inner.lock();
            Self::check_expiry(&mut inner);
            if inner.expirations > 0 {
                let val = inner.expirations;
                inner.expirations = 0;
                drop(inner);
                let mut writer = UserBufWriter::from(buf);
                writer.write_bytes(&val.to_ne_bytes())?;
                return Ok(8);
            }
            if options.nonblock {
                return Err(Errno::EAGAIN.into());
            }
        }

        // Slow path: block until timer fires.
        let val: u64 = POLL_WAIT_QUEUE.sleep_signalable_until(|| {
            let mut inner = self.inner.lock();
            Self::check_expiry(&mut inner);
            if inner.expirations > 0 {
                let val = inner.expirations;
                inner.expirations = 0;
                Ok(Some(val))
            } else {
                Ok(None)
            }
        })?;

        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&val.to_ne_bytes())?;
        Ok(8)
    }

    fn poll(&self) -> Result<PollStatus> {
        let mut inner = self.inner.lock();
        Self::check_expiry(&mut inner);
        if inner.expirations > 0 {
            Ok(PollStatus::POLLIN)
        } else {
            Ok(PollStatus::empty())
        }
    }
}
