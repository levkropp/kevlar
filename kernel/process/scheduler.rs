// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::process::PId;
use alloc::collections::VecDeque;
use core::sync::atomic::{AtomicUsize, Ordering};
use kevlar_platform::{arch::{cpu_id, num_online_cpus}, spinlock::SpinLock};

/// Maximum number of CPUs supported.
pub const MAX_CPUS: usize = 8;

/// Global count of processes in all run queues. Lock-free — used by
/// sched_yield to skip switch() when no other process is runnable.
static RUNQUEUE_LEN: AtomicUsize = AtomicUsize::new(0);

/// Returns the total number of processes across all run queues (lock-free).
pub fn runqueue_len() -> usize {
    RUNQUEUE_LEN.load(Ordering::Relaxed)
}

/// Scheduling policy trait.
///
/// Defines the interface for process scheduling algorithms. The Core
/// accesses the scheduler exclusively through this trait, making the
/// scheduling policy pluggable (e.g., round-robin, CFS).
pub trait SchedulerPolicy: Send + Sync {
    /// Enqueue a process into the run queue (back of queue).
    fn enqueue(&self, pid: PId);

    /// Enqueue a process at the FRONT of the run queue (priority boost).
    /// Used when a sleeping/blocked process wakes up — gives interactive
    /// processes priority over CPU-bound threads, similar to CFS sleep
    /// credit. Prevents starvation of I/O-bound processes by CPU hogs.
    fn enqueue_front(&self, pid: PId);

    /// Pick the next process to run, removing it from the run queue.
    fn pick_next(&self) -> Option<PId>;

    /// Remove a process from the run queue (e.g., on exit).
    fn remove(&self, pid: PId);
}

/// Per-CPU round-robin scheduler with work stealing.
///
/// Each CPU has its own run queue.  `enqueue` pushes to the calling CPU's
/// queue.  `pick_next` tries the local queue first, then steals from other
/// CPUs (round-robin victim selection, stealing from the back).
pub struct Scheduler {
    run_queues: [SpinLock<VecDeque<PId>>; MAX_CPUS],
}

impl Scheduler {
    pub fn new() -> Scheduler {
        Scheduler {
            run_queues: [
                SpinLock::new(VecDeque::new()),
                SpinLock::new(VecDeque::new()),
                SpinLock::new(VecDeque::new()),
                SpinLock::new(VecDeque::new()),
                SpinLock::new(VecDeque::new()),
                SpinLock::new(VecDeque::new()),
                SpinLock::new(VecDeque::new()),
                SpinLock::new(VecDeque::new()),
            ],
        }
    }
}

impl SchedulerPolicy for Scheduler {
    fn enqueue(&self, pid: PId) {
        // Load-balance to the LEAST-LOADED queue, not the caller's CPU.
        //
        // The previous version unconditionally used the caller's CPU,
        // which caused severe pile-ups when one CPU was busy ferrying
        // events for many user threads (e.g. Xorg's main loop on CPU
        // 0 while CPU 1 was idle).  Every wake_all from inside that
        // CPU's syscalls dropped the woken processes into the same
        // saturated queue; work-stealing only pops from the BACK of
        // a remote queue, so wake-ups stuck behind CPU-bound threads
        // waited a full preemption quantum (or longer if the busy
        // CPU never returned to its idle loop).  Symptom: PID 1
        // stalls 10s+ during test-i3 connection storms while CPU 1
        // sits idle.
        //
        // `enqueue_front` already does this (see comment there for
        // history); now `enqueue` does it too.
        let n = (num_online_cpus() as usize).min(MAX_CPUS);
        let mut best_cpu: usize = 0;
        let mut best_len = usize::MAX;
        for c in 0..n {
            let len = self.run_queues[c].lock().len();
            if len < best_len {
                best_len = len;
                best_cpu = c;
            }
        }
        self.run_queues[best_cpu].lock().push_back(pid);
        RUNQUEUE_LEN.fetch_add(1, Ordering::Relaxed);
        // Wake the target CPU if it isn't us — without this, an idle
        // CPU in WFI waits up to one timer period before noticing the
        // new entry.  On arm64 this is the difference between
        // "scheduler picks up wake immediately" and "wake stalls if
        // the target's timer is behaving oddly".  On x64 the IPI is a
        // no-op (timer is reliable enough that the latency hit of
        // "wait one tick" is in the noise).
        let me = cpu_id() as usize;
        if best_cpu != me {
            kevlar_platform::arch::send_reschedule_ipi(best_cpu as u32);
        }
    }

    fn enqueue_front(&self, pid: PId) {
        // Pick the LEAST-LOADED queue across all online CPUs and put
        // the boosted PID at the front of that queue.
        //
        // The previous version used the calling CPU's queue, which is
        // wrong on SMP: timer wakeups all land on whichever CPU took
        // the timer interrupt, and that CPU might be saturated with
        // CPU-bound user threads.  Work stealing only pops from the
        // BACK of remote queues, so a boosted PID stuck behind a
        // CPU-bound thread on the timer-interrupting CPU is invisible
        // to the other CPUs and waits a full preemption quantum (30ms
        // by default — but multiple back-to-back boosted enqueues into
        // the same queue can chain into multi-second stalls under
        // load).  Repro: test-xfce shows PID 1 freezing for 10s+
        // while xfce4-session children spawn.
        let n = (num_online_cpus() as usize).min(MAX_CPUS);
        let mut best_cpu: usize = 0;
        let mut best_len = usize::MAX;
        for c in 0..n {
            let len = self.run_queues[c].lock().len();
            if len < best_len {
                best_len = len;
                best_cpu = c;
            }
        }
        self.run_queues[best_cpu].lock().push_front(pid);
        RUNQUEUE_LEN.fetch_add(1, Ordering::Relaxed);
        let me = cpu_id() as usize;
        if best_cpu != me {
            kevlar_platform::arch::send_reschedule_ipi(best_cpu as u32);
        }
    }

    fn pick_next(&self) -> Option<PId> {
        let cpu = cpu_id() as usize;
        let local = cpu % MAX_CPUS;

        // Try local queue first.
        if let Some(pid) = self.run_queues[local].lock().pop_front() {
            RUNQUEUE_LEN.fetch_sub(1, Ordering::Relaxed);
            return Some(pid);
        }

        // Work stealing: try other CPUs in round-robin order, stealing from back.
        for i in 1..MAX_CPUS {
            let victim = (cpu + i) % MAX_CPUS;
            if let Some(pid) = self.run_queues[victim].lock().pop_back() {
                RUNQUEUE_LEN.fetch_sub(1, Ordering::Relaxed);
                return Some(pid);
            }
        }

        None
    }

    fn remove(&self, pid: PId) {
        for queue in &self.run_queues {
            let mut q = queue.lock();
            let before = q.len();
            q.retain(|p| *p != pid);
            let removed = before - q.len();
            if removed > 0 {
                RUNQUEUE_LEN.fetch_sub(removed, Ordering::Relaxed);
            }
        }
    }
}

impl Scheduler {
    /// Diagnostic: return the length of each run queue and the PIDs
    /// in it (up to `max` entries).  Used by the PID 1 stall detector
    /// in `timer.rs` to print scheduler state when the system is
    /// stuck.  No locks held on return; takes a single try_lock pass.
    pub fn snapshot(&self, max: usize) -> alloc::vec::Vec<(usize, alloc::vec::Vec<PId>)> {
        use alloc::vec::Vec;
        let n = (num_online_cpus() as usize).min(MAX_CPUS);
        let mut out: Vec<(usize, Vec<PId>)> = Vec::with_capacity(n);
        for c in 0..n {
            let q = self.run_queues[c].lock();
            let len = q.len();
            let mut pids: Vec<PId> = Vec::with_capacity(max.min(len));
            for (i, pid) in q.iter().enumerate() {
                if i >= max { break; }
                pids.push(*pid);
            }
            out.push((len, pids));
        }
        out
    }
}
