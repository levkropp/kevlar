// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::{
    ctypes::*,
    prelude::*,
    process::{self, current_process, Process, ProcessState},
};
use core::sync::atomic::{AtomicUsize, Ordering};
use kevlar_platform::{arch::TICK_HZ, spinlock::SpinLock};
use process::switch;

const PREEMPT_PER_TICKS: usize = 3;
static MONOTONIC_TICKS: AtomicUsize = AtomicUsize::new(0);
/// Ticks from the epoch (00:00:00 on 1 January 1970, UTC).
static WALLCLOCK_TICKS: AtomicUsize = AtomicUsize::new(0);
/// Wall-clock epoch in nanoseconds (set once from CMOS RTC at boot).
static WALLCLOCK_EPOCH_NS: AtomicUsize = AtomicUsize::new(0);
static TIMERS: SpinLock<Vec<Timer>> = SpinLock::new(Vec::new());

struct Timer {
    current: usize,
    process: Arc<Process>,
}

/// Suspends the current process at least `ms` milliseconds.
pub fn _sleep_ms(ms: usize) {
    TIMERS.lock().push(Timer {
        current: (ms * TICK_HZ + 999) / 1000,
        process: current_process().clone(),
    });

    current_process().set_state(ProcessState::BlockedSignalable);
    let _ = switch();
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct WallClock {
    ticks_from_epoch: usize,
}

impl WallClock {
    pub fn nanosecs_from_epoch(self) -> usize {
        // Base epoch (from CMOS RTC at boot) + ticks since boot.
        let base = WALLCLOCK_EPOCH_NS.load(Ordering::Relaxed);
        let tick_ns = self.ticks_from_epoch as u128 * 1_000_000_000 / TICK_HZ as u128;
        base + tick_ns as usize
    }

    pub fn secs_from_epoch(self) -> usize {
        self.nanosecs_from_epoch() / 1_000_000_000
    }

    pub fn msecs_from_epoch(self) -> usize {
        self.nanosecs_from_epoch() / 1_000_000
    }
}

pub fn read_wall_clock() -> WallClock {
    WallClock {
        ticks_from_epoch: WALLCLOCK_TICKS.load(Ordering::Relaxed),
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct MonotonicClock {
    ticks: usize,
    /// TSC-based nanosecond snapshot taken at creation time (x86_64 only).
    /// This allows `elapsed_msecs()` to compute real elapsed wall-clock
    /// time instead of always reading the current TSC.
    ns_snapshot: usize,
}

impl MonotonicClock {
    pub fn secs(self) -> usize {
        self.nanosecs() / 1_000_000_000
    }

    pub fn msecs(self) -> usize {
        self.nanosecs() / 1_000_000
    }

    pub fn nanosecs(self) -> usize {
        #[cfg(target_arch = "x86_64")]
        {
            if self.ns_snapshot != 0 {
                return self.ns_snapshot;
            }
        }
        // Fallback to tick-based timing.
        self.ticks * 1_000_000_000 / TICK_HZ
    }

    pub fn elapsed_msecs(self) -> usize {
        let now_ns = read_monotonic_clock().nanosecs();
        let self_ns = self.nanosecs();
        now_ns.saturating_sub(self_ns) / 1_000_000
    }
}

pub fn read_monotonic_clock() -> MonotonicClock {
    let ns = {
        #[cfg(target_arch = "x86_64")]
        {
            if kevlar_platform::arch::tsc::is_calibrated() {
                kevlar_platform::arch::tsc::nanoseconds_since_boot() as usize
            } else {
                0
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        { 0 }
    };
    MonotonicClock {
        ticks: MONOTONIC_TICKS.load(Ordering::Relaxed),
        ns_snapshot: ns,
    }
}

/// Returns the raw monotonic tick count (incremented once per timer IRQ).
pub fn monotonic_ticks() -> usize {
    MONOTONIC_TICKS.load(Ordering::Relaxed)
}

/// Initialize the wall-clock epoch from the platform's RTC.
/// Must be called early in boot, before any wall-clock queries.
pub fn init_wall_clock() {
    let epoch_secs = kevlar_platform::arch::read_rtc_epoch_secs();
    let epoch_ns = epoch_secs as usize * 1_000_000_000;
    WALLCLOCK_EPOCH_NS.store(epoch_ns, Ordering::Relaxed);
}

/// `struct timeval`
#[derive(Debug, Copy, Clone)]
#[repr(C, packed)]
pub struct Timeval {
    tv_sec: c_time,
    tv_usec: c_suseconds,
}

impl Timeval {
    pub fn new(tv_sec: c_time, tv_usec: c_suseconds) -> Self {
        Timeval { tv_sec, tv_usec }
    }

    pub fn as_msecs(&self) -> usize {
        (self.tv_sec as usize) * 1000 + (self.tv_usec as usize) / 1000
    }
}

/// Returns `true` if a context switch actually occurred (i.e. we switched to
/// a different thread).  The caller can use this to skip signal delivery using
/// the interrupted thread's frame — the new thread will receive signals on its
/// next preemption cycle.
pub fn handle_timer_irq() -> bool {
    crate::debug::htrace::enter(crate::debug::htrace::id::TIMER_IRQ, 0);
    {
        let mut timers = TIMERS.lock();
        for timer in timers.iter_mut() {
            if timer.current > 0 {
                timer.current -= 1;
            }
        }

        timers.retain(|timer| {
            if timer.current == 0 {
                timer.process.resume();
            }

            timer.current > 0
        })
    }

    // Tick real-time interval timers (setitimer/alarm → SIGALRM delivery).
    crate::syscalls::setitimer::tick_real_timers();

    // Wake poll/epoll/select waiters so they can re-check timeouts,
    // timerfd expirations, and signalfd readiness.
    crate::poll::POLL_WAIT_QUEUE.wake_all();

    // Approximate user-mode time: attribute the tick to whichever process
    // was running when the timer fired.
    {
        let proc = current_process();
        if !proc.is_idle() {
            proc.tick_utime();
        }
    }

    WALLCLOCK_TICKS.fetch_add(1, Ordering::Relaxed);
    let ticks = MONOTONIC_TICKS.fetch_add(1, Ordering::Relaxed);

    // Update VFS clock (second-level granularity for filesystem timestamps).
    kevlar_vfs::set_vfs_clock(read_wall_clock().secs_from_epoch() as u32);

    crate::debug::htrace::exit(crate::debug::htrace::id::TIMER_IRQ, 0);

    if ticks % PREEMPT_PER_TICKS == 0 && !kevlar_platform::arch::in_preempt() {
        return process::switch();
    }
    false
}
