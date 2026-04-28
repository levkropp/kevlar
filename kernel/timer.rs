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

/// LAPIC timer diagnostic: set to `true` to skip context switches from the
/// timer handler.  Timers, poll wakes, and tick accounting still run normally.
/// If the LAPIC heartbeat counters keep incrementing with this set to `true`,
/// the bug is in `switch()`'s interaction with the interrupt return path.
pub const DIAG_SKIP_SWITCH: bool = false;

/// Gate the per-second TICK_HB heartbeat warn! line that proves the timer
/// ISR is still running during a hang.  Useful when chasing a livelock,
/// noisy under normal operation (one line per second per CPU on the
/// console).  Flip to `true` when investigating a hang.
const TICK_HB_ENABLED: bool = false;

/// Gate the PID1_STALL detector that fires when PID 1 hasn't been observed
/// running on any CPU for >100 ticks (1s).  False-alarms when PID 1 is
/// deliberately blocked (e.g. test-lxde's interactive_keepalive sleeps in
/// 60s loops to keep the desktop alive in run-alpine-lxde) — the detector
/// can't tell "stuck" from "voluntarily idle".  Flip to `true` when
/// investigating a real init-process hang.
const PID1_STALL_ENABLED: bool = false;

static MONOTONIC_TICKS: AtomicUsize = AtomicUsize::new(0);
/// Per-CPU tick counter — bumped from `handle_timer_irq` regardless
/// of whether the global heartbeat fires.  Used to detect a dead
/// timer on a specific CPU (the AP virtual timer not re-arming).
static PER_CPU_TICKS: [AtomicUsize; 8] = [
    AtomicUsize::new(0), AtomicUsize::new(0),
    AtomicUsize::new(0), AtomicUsize::new(0),
    AtomicUsize::new(0), AtomicUsize::new(0),
    AtomicUsize::new(0), AtomicUsize::new(0),
];
/// Ticks from the epoch (00:00:00 on 1 January 1970, UTC).
static WALLCLOCK_TICKS: AtomicUsize = AtomicUsize::new(0);
/// Wall-clock epoch in nanoseconds (set once from CMOS RTC at boot).
static WALLCLOCK_EPOCH_NS: AtomicUsize = AtomicUsize::new(0);
/// Task #25 diagnostic: track the last tick at which PID 1 was observed
/// running on any CPU.  The timer ISR compares against MONOTONIC_TICKS
/// and fires a one-shot starvation dump if PID 1 hasn't run in more
/// than 5 seconds — during which it prints scheduler run queue
/// lengths, the TIMERS list length, and attempts to identify whatever
/// is spinning instead.  Flag below gates the print so we only fire
/// once per stall event (re-armed when PID 1 runs again).
pub static PID1_LAST_TICK: AtomicUsize = AtomicUsize::new(0);
static PID1_STALL_DUMPED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
const PID1_STALL_THRESHOLD_TICKS: usize = 500;  // 5 seconds at 100 Hz

/// Re-arm the PID 1 stall dump so that the next stall event will
/// print again.  Called from switch() whenever PID 1 becomes CURRENT.
pub fn pid1_stall_rearm() {
    PID1_STALL_DUMPED.store(false, Ordering::Relaxed);
}
static TIMERS: SpinLock<Vec<Timer>> = SpinLock::new_ranked(
    Vec::new(),
    kevlar_platform::lockdep::rank::TIMERS,
    "TIMERS",
);

struct Timer {
    current: usize,
    process: Arc<Process>,
}

/// Suspends the current process at least `ms` milliseconds.
///
/// The push-into-TIMERS and the state transition to `BlockedSignalable`
/// happen under the same TIMERS lock — without that, a timer IRQ firing
/// between the push and the `set_state` could observe our Timer entry,
/// decrement-and-expire it, and call `resume_boosted()` on us while our
/// state is still `Runnable`.  Our subsequent `set_state(Blocked)`
/// then removes us from the run queue — and because the Timer entry
/// was already consumed by the IRQ, nothing will ever wake us again
/// through this sleep path.  We'd sleep until some unrelated signal
/// (SIGCHLD from a child exit) happens to rescue us.
///
/// With IF=0 syscall bodies this race couldn't happen (the timer IRQ
/// can't fire between `lock().push()` and `set_state`).  With broad
/// `sti` in syscall_entry it becomes reachable on every nanosleep call,
/// manifesting as ~186ms average sleep time for a requested 50ms —
/// the smoking gun the latency histogram from commit f84193e surfaced.
pub fn _sleep_ms(ms: usize) {
    let ticks = (ms * TICK_HZ + 999) / 1000;
    {
        let mut timers = TIMERS.lock();
        timers.push(Timer {
            current: ticks,
            process: current_process().clone(),
        });
        // Still under the TIMERS lock: transition to BlockedSignalable
        // before a timer IRQ can decrement our `current` counter.
        current_process().set_state(ProcessState::BlockedSignalable);
    }
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
        if self.ns_snapshot != 0 {
            return self.ns_snapshot;
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
        #[cfg(target_arch = "aarch64")]
        {
            kevlar_platform::arch::nanoseconds_since_boot() as usize
        }
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
    // NMI watchdog: periodically check if any CPU's LAPIC heartbeat has stalled.
    kevlar_platform::arch::watchdog_check();

    crate::debug::htrace::enter(crate::debug::htrace::id::TIMER_IRQ, 0);
    // Collect expired timers while holding the lock, then resume them
    // AFTER releasing the lock. This breaks the TIMERS→SCHEDULER lock
    // nesting that caused a deadlock with switch()→SCHEDULER on SMP.
    let expired: alloc::vec::Vec<Arc<Process>> = {
        let mut timers = TIMERS.lock();
        for timer in timers.iter_mut() {
            if timer.current > 0 {
                timer.current -= 1;
            }
        }
        let mut expired = alloc::vec::Vec::new();
        timers.retain(|timer| {
            if timer.current == 0 {
                expired.push(timer.process.clone());
                false
            } else {
                true
            }
        });
        expired
    }; // TIMERS lock released here

    // Now resume expired timers without holding TIMERS lock.
    for proc in &expired {
        proc.resume_boosted();
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

    // Per-CPU tick counter — exposes whether each CPU's timer is alive.
    // If one CPU's count is stuck at the boot value, its timer has died
    // (e.g. arm64 virtual timer on AP not rearming) and that CPU can't
    // pick up runnable work even if its queue is non-empty.
    let cpu_idx = kevlar_platform::arch::cpu_id() as usize;
    if cpu_idx < PER_CPU_TICKS.len() {
        PER_CPU_TICKS[cpu_idx].fetch_add(1, Ordering::Relaxed);
    }

    // Task #25 diagnostic: per-second tick heartbeat + PID 1
    // starvation detector.  The heartbeat proves the timer ISR is
    // still running during a hang.  Gated by TICK_HB_ENABLED — flip
    // it to `true` when chasing a hang; otherwise the line is a
    // per-second per-CPU console spam.
    if TICK_HB_ENABLED && ticks % 100 == 0 {
        let last = PID1_LAST_TICK.load(Ordering::Relaxed);
        let gap = ticks.saturating_sub(last);
        let cpu = kevlar_platform::arch::cpu_id();
        let pcs: alloc::vec::Vec<usize> = PER_CPU_TICKS.iter()
            .map(|a| a.load(Ordering::Relaxed)).collect();
        warn!("TICK_HB: cpu={} tick={} pid1_last={} pid1_gap={} per_cpu={:?}",
              cpu, ticks, last, gap, pcs);
    }
    if PID1_STALL_ENABLED && ticks % 50 == 25 {  // twice per second, offset from preempt
        let last = PID1_LAST_TICK.load(Ordering::Relaxed);
        // Dump on every sample once gap > 100 ticks (1s).  Repeated
        // samples let us tell whether the userspace PC is stuck (a
        // loop) or moving (forward progress).
        if last != 0 && ticks > last + 100 {
            let gap_ms = (ticks - last) * 1000 / TICK_HZ;
            #[cfg(target_arch = "aarch64")]
            let fp_traps = kevlar_platform::arch::arm64_specific::FP_TRAP_COUNT_VALUE();
            #[cfg(not(target_arch = "aarch64"))]
            let fp_traps: usize = 0;
            let pf_count = crate::mm::page_fault::PAGE_FAULT_COUNT
                .load(Ordering::Relaxed);
            for cpu in 0..(kevlar_platform::arch::num_online_cpus() as usize) {
                if let Some((pc, sp, _pstate, stale)) = kevlar_platform::arch::last_user_state(cpu) {
                    // stale=0 → CPU is currently running EL0 (or just
                    // trapped from it in this IRQ).  Higher = N IRQs
                    // have fired in EL1/idle since we last sampled.
                    let regs = kevlar_platform::arch::last_user_regs(cpu);
                    let (lr, x0, x1, x2) = regs.unwrap_or((0, 0, 0, 0));
                    warn!(
                        "PID1_STALL: tick={} gap={}ms cpu={} stale_irqs={} user_pc=0x{:x} sp=0x{:x} lr=0x{:x} x0=0x{:x} x1=0x{:x} x2=0x{:x} fp_traps={} pf={}",
                        ticks, gap_ms, cpu, stale, pc, sp, lr, x0, x1, x2, fp_traps, pf_count,
                    );
                    // Read the 16 bytes at LR-8..LR+8 from user memory.
                    // The instruction at LR-4 is the call site that
                    // dispatched into where we're now sampled (if we're
                    // currently inside a function that x30 returns from).
                    if stale == 0 && lr >= 8 {
                        let mut buf = [0u8; 16];
                        let uva = kevlar_platform::address::UserVAddr::new_nonnull(
                            (lr - 8) as usize,
                        );
                        if let Ok(uva) = uva {
                            if uva.read_bytes(&mut buf).is_ok() {
                                let i_pre = u32::from_le_bytes([buf[0],buf[1],buf[2],buf[3]]);
                                let i_call = u32::from_le_bytes([buf[4],buf[5],buf[6],buf[7]]);
                                let i_at = u32::from_le_bytes([buf[8],buf[9],buf[10],buf[11]]);
                                let i_next = u32::from_le_bytes([buf[12],buf[13],buf[14],buf[15]]);
                                warn!(
                                    "PID1_STALL: cpu={} insns near lr-8: {:08x} {:08x} {:08x} {:08x}",
                                    cpu, i_pre, i_call, i_at, i_next,
                                );
                            }
                        }
                    }
                }
            }
        }
        if last != 0 && ticks > last + PID1_STALL_THRESHOLD_TICKS
            && !PID1_STALL_DUMPED.swap(true, Ordering::Relaxed)
        {
            let queues = crate::process::dump_scheduler_state(4);
            warn!("PID1_STALL queues={:?}", queues);
        }
    }


    // Update VFS clock with nanosecond precision for filesystem timestamps.
    let wall_ns = read_wall_clock().nanosecs_from_epoch();
    let secs = (wall_ns / 1_000_000_000) as u32;
    let nsec = (wall_ns % 1_000_000_000) as u32;
    kevlar_vfs::set_vfs_clock_ns(secs, nsec);

    crate::debug::htrace::exit(crate::debug::htrace::id::TIMER_IRQ, 0);

    if ticks % PREEMPT_PER_TICKS == 0 {
        if DIAG_SKIP_SWITCH {
            // Diagnostic mode: skip context switches entirely.
            // Timers, poll wakes, and tick accounting still run.
            return false;
        }
        if !kevlar_platform::arch::in_preempt() {
            return process::switch();
        }
        // Preemption is disabled (lock_preempt held). Defer the reschedule
        // to when preempt_enable() is called — this prevents timer starvation
        // when CPU-bound threads hold preemption locks in tight loops.
        kevlar_platform::arch::set_need_resched();
    }
    false
}
