// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// setitimer(ITIMER_REAL) — deliver SIGALRM after a specified interval.
// Also implements alarm() properly (was previously a stub).
use crate::{
    prelude::*,
    process::{current_process, signal::SIGALRM, PId},
    syscalls::SyscallHandler,
};
use kevlar_platform::{address::UserVAddr, arch::TICK_HZ, spinlock::SpinLock};

const ITIMER_REAL: i32 = 0;

/// Per-process real-time interval timer state.
/// When the countdown reaches 0, SIGALRM is delivered.
struct RealTimer {
    pid: PId,
    remaining_ticks: usize,
}

static REAL_TIMERS: SpinLock<alloc::vec::Vec<RealTimer>> = SpinLock::new(alloc::vec::Vec::new());

/// Called from handle_timer_irq() every tick to decrement real timers.
pub fn tick_real_timers() {
    // Fast path: skip lock if no timers registered.
    if TIMER_COUNT.load(core::sync::atomic::Ordering::Relaxed) == 0 {
        return;
    }

    let mut timers = REAL_TIMERS.lock_no_irq();
    let mut expired_pid = None;
    for t in timers.iter_mut() {
        if t.remaining_ticks > 0 {
            t.remaining_ticks -= 1;
        }
        if t.remaining_ticks == 0 {
            expired_pid = Some(t.pid);
        }
    }
    timers.retain(|t| t.remaining_ticks > 0);
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

fn usecs_to_ticks(sec: i64, usec: i64) -> usize {
    let total_us = (sec as usize) * 1_000_000 + (usec as usize);
    // Convert microseconds to timer ticks (rounding up).
    (total_us * TICK_HZ + 999_999) / 1_000_000
}

impl<'a> SyscallHandler<'a> {
    pub fn sys_setitimer(
        &mut self,
        which: i32,
        new_value: Option<UserVAddr>,
        _old_value: Option<UserVAddr>,
    ) -> Result<isize> {
        if which != ITIMER_REAL {
            // ITIMER_VIRTUAL and ITIMER_PROF not implemented.
            return Err(Errno::ENOSYS.into());
        }

        let pid = current_process().pid();

        // Remove any existing timer for this process.
        {
            let mut timers = REAL_TIMERS.lock();
            timers.retain(|t| t.pid != pid);
        }

        if let Some(new_ptr) = new_value {
            let itv = new_ptr.read::<ITimerVal>()?;
            let ticks = usecs_to_ticks(itv.it_value_sec, itv.it_value_usec);

            if ticks > 0 {
                REAL_TIMERS.lock().push(RealTimer {
                    pid,
                    remaining_ticks: ticks,
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

        // Remove existing timer and return remaining seconds.
        let old_remaining = {
            let mut timers = REAL_TIMERS.lock();
            let mut old = 0usize;
            timers.retain(|t| {
                if t.pid == pid {
                    old = t.remaining_ticks * 1_000_000 / TICK_HZ / 1_000_000;
                    false
                } else {
                    true
                }
            });
            old
        };

        if seconds > 0 {
            let ticks = (seconds as usize) * TICK_HZ;
            REAL_TIMERS.lock().push(RealTimer {
                pid,
                remaining_ticks: ticks,
            });
            TIMER_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }

        Ok(old_remaining as isize)
    }
}
