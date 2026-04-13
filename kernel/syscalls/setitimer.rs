// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// setitimer(ITIMER_REAL) — deliver SIGALRM after a specified interval.
// Also implements alarm() properly (was previously a stub).
//
// Timers use absolute nanosecond deadlines (TSC-backed monotonic clock)
// for sub-microsecond precision, matching Linux hrtimer behaviour.
use crate::{
    prelude::*,
    process::{current_process, signal::SIGALRM, PId},
    syscalls::SyscallHandler,
    timer::read_monotonic_clock,
};
use kevlar_platform::{address::UserVAddr, spinlock::SpinLock};

const ITIMER_REAL: i32 = 0;

/// Per-process real-time interval timer state.
/// Deadline is an absolute monotonic nanosecond timestamp.
struct RealTimer {
    pid: PId,
    deadline_ns: u64,
}

static REAL_TIMERS: SpinLock<alloc::vec::Vec<RealTimer>> = SpinLock::new_ranked(
    alloc::vec::Vec::new(),
    kevlar_platform::lockdep::rank::REAL_TIMERS,
    "REAL_TIMERS",
);

/// Current monotonic time in nanoseconds.
#[inline]
fn now_ns() -> u64 {
    read_monotonic_clock().nanosecs() as u64
}

/// Called from handle_timer_irq() every tick to check real timer deadlines.
pub fn tick_real_timers() {
    // Fast path: skip lock if no timers registered.
    if TIMER_COUNT.load(core::sync::atomic::Ordering::Relaxed) == 0 {
        return;
    }

    let now = now_ns();
    let mut timers = REAL_TIMERS.lock_no_irq();
    let mut expired_pid = None;
    timers.retain(|t| {
        if now >= t.deadline_ns {
            expired_pid = Some(t.pid);
            false
        } else {
            true
        }
    });
    drop(timers);

    // Deliver SIGALRM outside the REAL_TIMERS lock to avoid nested locks.
    if let Some(pid) = expired_pid {
        TIMER_COUNT.fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
        if let Some(proc) = crate::process::Process::find_by_pid(pid) {
            proc.send_signal(SIGALRM);
        }
    }
}

static TIMER_COUNT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// itimerval structure layout (Linux x86_64):
///   it_interval: timeval (16 bytes) — repeat interval (0 = one-shot)
///   it_value:    timeval (16 bytes) — initial countdown
/// Each timeval: tv_sec (8 bytes) + tv_usec (8 bytes)
#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct ITimerVal {
    it_interval_sec: i64,
    it_interval_usec: i64,
    it_value_sec: i64,
    it_value_usec: i64,
}

/// Convert a timeval (sec, usec) to nanoseconds.
fn timeval_to_ns(sec: i64, usec: i64) -> u64 {
    (sec as u64) * 1_000_000_000 + (usec as u64) * 1_000
}

impl<'a> SyscallHandler<'a> {
    pub fn sys_getitimer(&mut self, which: i32, curr_value: Option<UserVAddr>) -> Result<isize> {
        if which != ITIMER_REAL {
            return Err(Errno::ENOSYS.into());
        }

        let Some(out_ptr) = curr_value else {
            return Err(Errno::EFAULT.into());
        };

        let pid = current_process().pid();
        let now = now_ns();

        let remaining_ns = {
            let timers = REAL_TIMERS.lock_no_irq();
            timers.iter()
                .find(|t| t.pid == pid)
                .map(|t| t.deadline_ns.saturating_sub(now))
                .unwrap_or(0)
        };

        let us = remaining_ns / 1_000;
        let itv = ITimerVal {
            it_interval_sec: 0,
            it_interval_usec: 0,
            it_value_sec: (us / 1_000_000) as i64,
            it_value_usec: (us % 1_000_000) as i64,
        };
        out_ptr.write::<ITimerVal>(&itv)?;
        Ok(0)
    }

    pub fn sys_setitimer(
        &mut self,
        which: i32,
        new_value: Option<UserVAddr>,
        old_value: Option<UserVAddr>,
    ) -> Result<isize> {
        if which != ITIMER_REAL {
            // ITIMER_VIRTUAL and ITIMER_PROF not implemented.
            return Err(Errno::ENOSYS.into());
        }

        let pid = current_process().pid();
        let now = now_ns();

        // Remove any existing timer for this process and capture old remaining.
        let old_remaining_ns = {
            let mut timers = REAL_TIMERS.lock();
            let mut old_ns = 0u64;
            let mut removed = false;
            timers.retain(|t| {
                if t.pid == pid {
                    old_ns = t.deadline_ns.saturating_sub(now);
                    removed = true;
                    false
                } else {
                    true
                }
            });
            if removed {
                TIMER_COUNT.fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
            }
            old_ns
        };

        // Write old value back to userspace if requested.
        if let Some(old_ptr) = old_value {
            let old_us = old_remaining_ns / 1_000;
            let old_itv = ITimerVal {
                it_interval_sec: 0,
                it_interval_usec: 0,
                it_value_sec: (old_us / 1_000_000) as i64,
                it_value_usec: (old_us % 1_000_000) as i64,
            };
            old_ptr.write::<ITimerVal>(&old_itv)?;
        }

        if let Some(new_ptr) = new_value {
            let itv = new_ptr.read::<ITimerVal>()?;
            let interval_ns = timeval_to_ns(itv.it_value_sec, itv.it_value_usec);

            if interval_ns > 0 {
                REAL_TIMERS.lock().push(RealTimer {
                    pid,
                    deadline_ns: now + interval_ns,
                });
                TIMER_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            }
            // Note: it_interval (repeat) is ignored for now — one-shot only.
            // This is sufficient for the sa_restart contract test.
        }

        Ok(0)
    }

    pub fn sys_alarm(&mut self, seconds: u32) -> Result<isize> {
        let pid = current_process().pid();
        let now = now_ns();

        // Remove existing timer and return remaining seconds (rounded up).
        let old_remaining_secs = {
            let mut timers = REAL_TIMERS.lock();
            let mut old_ns = 0u64;
            let mut removed = false;
            timers.retain(|t| {
                if t.pid == pid {
                    old_ns = t.deadline_ns.saturating_sub(now);
                    removed = true;
                    false
                } else {
                    true
                }
            });
            if removed {
                TIMER_COUNT.fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
            }
            ((old_ns + 999_999_999) / 1_000_000_000) as usize
        };

        if seconds > 0 {
            let deadline = now + (seconds as u64) * 1_000_000_000;
            REAL_TIMERS.lock().push(RealTimer {
                pid,
                deadline_ns: deadline,
            });
            TIMER_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }

        Ok(old_remaining_secs as isize)
    }
}
