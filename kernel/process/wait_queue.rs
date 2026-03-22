// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use super::{current_process, switch, Process, ProcessState};
use crate::result::Errno;
use crate::result::Result;

use alloc::{collections::VecDeque, sync::Arc};
use core::sync::atomic::{AtomicUsize, Ordering};
use kevlar_platform::spinlock::SpinLock;

pub struct WaitQueue {
    queue: SpinLock<VecDeque<Arc<Process>>>,
    waiter_count: AtomicUsize,
}

impl WaitQueue {
    pub fn new() -> WaitQueue {
        WaitQueue {
            queue: SpinLock::new(VecDeque::new()),
            waiter_count: AtomicUsize::new(0),
        }
    }

    pub fn waiter_count(&self) -> usize {
        self.waiter_count.load(Ordering::Relaxed)
    }

    /// Sleeps until `condition` returns `Some`, or a signal arrives (EINTR).
    ///
    /// Optimized vs previous version:
    /// - Checks condition BEFORE enqueueing (fast path if already met)
    /// - After waking, checks condition BEFORE re-enqueueing
    /// - wake_all removes from queue; sleeper only self-removes on signal abort
    pub fn sleep_signalable_until<F, R>(&self, mut condition: F) -> Result<R>
    where
        F: FnMut() -> Result<Option<R>>,
    {
        use crate::debug::htrace;
        let _htrace_guard = htrace::enter_guard(htrace::id::SLEEP_UNTIL, 0);

        // Fast path: condition already met — no queue ops at all.
        htrace::enter(htrace::id::SLEEP_CALLBACK, 0);
        let fast = condition();
        htrace::exit(htrace::id::SLEEP_CALLBACK, 0);
        match fast {
            Ok(Some(result)) => return Ok(result),
            Err(e) => return Err(e),
            Ok(None) => {}
        }

        loop {
            if current_process().has_pending_signals() {
                return Err(Errno::EINTR.into());
            }

            // Enqueue and sleep. Hold queue lock across state change + push
            // to prevent lost-wakeup race with preemption timer.
            {
                let mut q = self.queue.lock();
                current_process().set_state(ProcessState::BlockedSignalable);
                q.push_back(current_process().clone());
                self.waiter_count.fetch_add(1, Ordering::Relaxed);
            }

            #[cfg(feature = "ktrace-sched")]
            crate::debug::ktrace::trace(crate::debug::ktrace::event::WAITQ_SLEEP,
                self as *const _ as u32, 0, 0, 0, 0);

            // Yield CPU. We'll be woken by wake_all/wake_one which removes
            // us from the queue and sets us Runnable.
            switch();

            // After waking: check condition FIRST, then signals. This is
            // critical for wait4: SIGCHLD wakes us AND the child is now a
            // zombie. If we checked signals first, we'd return EINTR without
            // ever seeing the exited child.

            htrace::enter(htrace::id::SLEEP_CALLBACK, 1);
            let result = condition();
            htrace::exit(htrace::id::SLEEP_CALLBACK, 1);
            match result {
                Ok(Some(result)) => return Ok(result),
                Err(e) => return Err(e),
                Ok(None) => {} // condition not met — check if a signal woke us
            }

            if current_process().has_pending_signals() {
                // Signal woke us (not wake_all). We might still be in the
                // queue if the signal arrived via the interrupt return path.
                // Self-remove if still present.
                self.try_remove_current();
                return Err(Errno::EINTR.into());
            }
        }
    }

    /// Remove current process from queue if present (idempotent).
    fn try_remove_current(&self) {
        let mut q = self.queue.lock();
        let before = q.len();
        q.retain(|p| !Arc::ptr_eq(p, current_process()));
        let removed = before - q.len();
        if removed > 0 {
            self.waiter_count.fetch_sub(removed, Ordering::Relaxed);
        }
    }

    pub fn _wake_one(&self) {
        if self.waiter_count.load(Ordering::Relaxed) == 0 {
            return;
        }
        let mut queue = self.queue.lock();
        if let Some(process) = queue.pop_front() {
            self.waiter_count.fetch_sub(1, Ordering::Relaxed);
            process.resume();
        }
    }

    pub fn wake_all(&self) {
        if self.waiter_count.load(Ordering::Relaxed) == 0 {
            return;
        }
        use crate::debug::htrace;
        htrace::enter(htrace::id::WAKE_ALL, self.waiter_count.load(Ordering::Relaxed) as u32);
        let mut queue = self.queue.lock();
        let mut woken_count = 0u32;
        while let Some(process) = queue.pop_front() {
            self.waiter_count.fetch_sub(1, Ordering::Relaxed);
            process.resume();
            woken_count += 1;
        }
        htrace::exit(htrace::id::WAKE_ALL, 0);

        #[cfg(feature = "ktrace-sched")]
        crate::debug::ktrace::trace(crate::debug::ktrace::event::WAITQ_WAKE,
            self as *const _ as u32, woken_count, 0, 0, 0);
    }

    pub fn wake_n(&self, max: u32) -> u32 {
        if self.waiter_count.load(Ordering::Relaxed) == 0 || max == 0 {
            return 0;
        }
        let mut queue = self.queue.lock();
        let mut woken = 0u32;
        while woken < max {
            if let Some(process) = queue.pop_front() {
                self.waiter_count.fetch_sub(1, Ordering::Relaxed);
                process.resume();
                woken += 1;
            } else {
                break;
            }
        }
        woken
    }

    pub fn requeue_to(&self, other: &WaitQueue, max: usize) -> usize {
        if self.waiter_count.load(Ordering::Relaxed) == 0 || max == 0 {
            return 0;
        }
        let mut src = self.queue.lock();
        let mut dst = other.queue.lock();
        let mut moved = 0usize;
        while moved < max {
            if let Some(process) = src.pop_front() {
                self.waiter_count.fetch_sub(1, Ordering::Relaxed);
                dst.push_back(process);
                other.waiter_count.fetch_add(1, Ordering::Relaxed);
                moved += 1;
            } else {
                break;
            }
        }
        moved
    }
}
