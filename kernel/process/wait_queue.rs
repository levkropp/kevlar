// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use super::{current_process, switch, Process, ProcessState};
use crate::result::Errno;
use crate::result::Result;

use alloc::{collections::VecDeque, sync::Arc};
use core::sync::atomic::{AtomicUsize, Ordering};
use kevlar_platform::spinlock::SpinLock;

pub struct WaitQueue {
    queue: SpinLock<VecDeque<Arc<Process>>>,
    /// Number of processes currently enqueued.  Checked with a relaxed load
    /// to skip the lock in `wake_all` when nobody is waiting.
    waiter_count: AtomicUsize,
}

impl WaitQueue {
    pub fn new() -> WaitQueue {
        WaitQueue {
            queue: SpinLock::new(VecDeque::new()),
            waiter_count: AtomicUsize::new(0),
        }
    }

    /// Sleeps on the wait queue until `sleep_if_none` returns `Some`.
    ///
    /// If a signal is arrived, this method returns `Err(Errno::EINTR)`.
    pub fn sleep_signalable_until<F, R>(&self, mut sleep_if_none: F) -> Result<R>
    where
        F: FnMut() -> Result<Option<R>>,
    {
        loop {
            // Atomically set state to BlockedSignalable AND enqueue in the
            // wait queue while holding the queue's SpinLock (which disables
            // interrupts via cli).  Without this, the LAPIC preempt timer can
            // fire in the window between set_state() and push_back():
            //
            //   set_state(BlockedSignalable)  ← removed from run queue
            //   [LAPIC preempt fires here]    ← switch() sees Blocked, does
            //                                    NOT re-enqueue → thread lost!
            //   push_back(current)            ← never reached
            //
            // A lost thread (neither in the run queue nor any WaitQueue) will
            // never be resumed, causing the joining thread to block forever.
            // Holding the queue lock across both operations keeps interrupts
            // masked for those ~2 instructions, preventing the race.
            {
                let mut q = self.queue.lock();
                current_process().set_state(ProcessState::BlockedSignalable);
                q.push_back(current_process().clone());
                self.waiter_count.fetch_add(1, Ordering::Relaxed);
            }

            if current_process().has_pending_signals() {
                current_process().set_state(ProcessState::Runnable);
                self.queue
                    .lock()
                    .retain(|proc| !Arc::ptr_eq(proc, current_process()));
                self.waiter_count.fetch_sub(1, Ordering::Relaxed);
                return Err(Errno::EINTR.into());
            }

            let ret_value = match sleep_if_none() {
                Ok(Some(ret_value)) => Some(Ok(ret_value)),
                Ok(None) => None,
                Err(err) => Some(Err(err)),
            };

            if let Some(ret_value) = ret_value {
                // The condition is met. The current thread doesn't have to sleep.
                // Same reasoning: set_state(Runnable) rather than resume() to
                // avoid spuriously enqueuing the currently-running process.
                current_process().set_state(ProcessState::Runnable);
                self.queue
                    .lock()
                    .retain(|proc| !Arc::ptr_eq(proc, current_process()));
                self.waiter_count.fetch_sub(1, Ordering::Relaxed);
                return ret_value;
            }

            // Run other threads until someone wake us up...
            switch();

            // Check for pending signals immediately after waking.
            // This catches signals that were delivered via the interrupt
            // return path (try_delivering_signal) while we were blocked.
            if current_process().has_pending_signals() {
                current_process().set_state(ProcessState::Runnable);
                self.queue
                    .lock()
                    .retain(|proc| !Arc::ptr_eq(proc, current_process()));
                self.waiter_count.fetch_sub(1, Ordering::Relaxed);
                return Err(Errno::EINTR.into());
            }
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
        let mut queue = self.queue.lock();
        while let Some(process) = queue.pop_front() {
            self.waiter_count.fetch_sub(1, Ordering::Relaxed);
            process.resume();
        }
    }

    /// Wake up to `max` waiters. Returns the number actually woken.
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

    /// Move up to `max` waiters from `self` to `other` without waking them.
    /// Returns the number of waiters moved.
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
