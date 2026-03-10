// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::process::PId;
use alloc::collections::VecDeque;
use kevlar_platform::{arch::cpu_id, spinlock::SpinLock};

/// Maximum number of CPUs supported.
pub const MAX_CPUS: usize = 8;

/// Scheduling policy trait.
///
/// Defines the interface for process scheduling algorithms. The Core
/// accesses the scheduler exclusively through this trait, making the
/// scheduling policy pluggable (e.g., round-robin, CFS).
pub trait SchedulerPolicy: Send + Sync {
    /// Enqueue a process into the run queue.
    fn enqueue(&self, pid: PId);

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
    }

    fn pick_next(&self) -> Option<PId> {
        let cpu = cpu_id() as usize;
        let local = cpu % MAX_CPUS;

        // Try local queue first.
        if let Some(pid) = self.run_queues[local].lock().pop_front() {
            return Some(pid);
        }

        // Work stealing: try other CPUs in round-robin order, stealing from back.
        for i in 1..MAX_CPUS {
            let victim = (cpu + i) % MAX_CPUS;
            if let Some(pid) = self.run_queues[victim].lock().pop_back() {
                return Some(pid);
            }
        }

        None
    }

    fn remove(&self, pid: PId) {
        for queue in &self.run_queues {
            queue.lock().retain(|p| *p != pid);
        }
    }
}
