// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::process::PId;
use alloc::collections::VecDeque;
use kevlar_platform::spinlock::SpinLock;

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

/// Round-robin scheduler.
pub struct Scheduler {
    run_queue: SpinLock<VecDeque<PId>>,
}

impl Scheduler {
    /// Creates a scheduler.
    pub fn new() -> Scheduler {
        Scheduler {
            run_queue: SpinLock::new(VecDeque::new()),
        }
    }
}

impl SchedulerPolicy for Scheduler {
    fn enqueue(&self, pid: PId) {
        self.run_queue.lock().push_back(pid);
    }

    fn pick_next(&self) -> Option<PId> {
        self.run_queue.lock().pop_front()
    }

    fn remove(&self, pid: PId) {
        self.run_queue.lock().retain(|p| *p != pid);
    }
}
