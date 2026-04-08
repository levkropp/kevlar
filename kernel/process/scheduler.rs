// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::process::PId;
use alloc::collections::VecDeque;
use core::sync::atomic::{AtomicUsize, Ordering};
use kevlar_platform::{arch::cpu_id, spinlock::SpinLock};

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
        let cpu = cpu_id() as usize % MAX_CPUS;
        self.run_queues[cpu].lock().push_back(pid);
        RUNQUEUE_LEN.fetch_add(1, Ordering::Relaxed);
    }

    fn enqueue_front(&self, pid: PId) {
        let cpu = cpu_id() as usize % MAX_CPUS;
        self.run_queues[cpu].lock().push_front(pid);
        RUNQUEUE_LEN.fetch_add(1, Ordering::Relaxed);
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
