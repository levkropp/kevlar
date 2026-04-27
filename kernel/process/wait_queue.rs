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

        self.sleep_signalable_until_inner(condition)
    }

    /// Like `sleep_signalable_until` but skips the initial condition check.
    /// Use when the caller already verified the condition is not met, to
    /// avoid a redundant (and expensive) re-check under lock.
    pub fn sleep_signalable_until_unchecked<F, R>(&self, condition: F) -> Result<R>
    where
        F: FnMut() -> Result<Option<R>>,
    {
        self.sleep_signalable_until_inner(condition)
    }

    fn sleep_signalable_until_inner<F, R>(&self, mut condition: F) -> Result<R>
    where
        F: FnMut() -> Result<Option<R>>,
    {
        use crate::debug::htrace;
        loop {
            if current_process().has_pending_signals() {
                return Err(Errno::EINTR.into());
            }

            // Enqueue *before* re-checking the condition.  Previously this
            // function checked the condition via the fast path in
            // `sleep_signalable_until`, then (if not ready) entered the
            // loop and enqueued — creating a classic lost-wakeup window:
            // a concurrent `wake_all()` between the fast-path check and
            // the enqueue would observe `waiter_count == 0`, do nothing,
            // and leave this sleeper waiting for a second wake that may
            // never arrive.
            //
            // Symptom: AF_UNIX client ppoll() returns EAGAIN, then
            // enqueues; peer writes in that gap, calls wake_all with no
            // waiters; client then sleeps forever.  Observed on arm64
            // HVF as xsetroot/xterm/Xorg hangs in blog 229 debugging.
            //
            // Correct ordering:
            //   1. Enqueue self with state=BlockedSignalable.
            //   2. Re-check condition.  If ready, self-remove and return
            //      — any write that landed after step 1 now sees us on
            //      the queue and will wake us, but we don't need the
            //      wake anymore.  Either way we observe the change.
            //   3. Only if condition is still not met, switch().
            {
                let _s = crate::debug::tracer::span_guard(
                    crate::debug::tracer::span::SLEEP_ENQUEUE);
                let mut q = self.queue.lock();
                current_process().set_state(ProcessState::BlockedSignalable);
                q.push_back(current_process().clone());
                self.waiter_count.fetch_add(1, Ordering::Relaxed);
            }

            // Re-check the condition now that we're enqueued.  This
            // plugs the lost-wakeup race: any wake_all from here on is
            // guaranteed to find us in the queue.
            htrace::enter(htrace::id::SLEEP_CALLBACK, 2);
            let recheck = condition();
            htrace::exit(htrace::id::SLEEP_CALLBACK, 2);
            match recheck {
                Ok(Some(result)) => {
                    self.try_remove_current();
                    current_process().set_state(ProcessState::Runnable);
                    return Ok(result);
                }
                Err(e) => {
                    self.try_remove_current();
                    current_process().set_state(ProcessState::Runnable);
                    return Err(e);
                }
                Ok(None) => {} // still not ready — fall through to switch()
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

            let _resume_span = crate::debug::tracer::span_guard(
                crate::debug::tracer::span::SLEEP_RESUME);
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

    pub fn wake_one(&self) {
        if self.waiter_count.load(Ordering::Relaxed) == 0 {
            return;
        }
        let process = {
            let mut queue = self.queue.lock();
            let p = queue.pop_front();
            if p.is_some() { self.waiter_count.fetch_sub(1, Ordering::Relaxed); }
            p
        };
        if let Some(process) = process {
            process.resume();
        }
    }

    pub fn wake_all(&self) {
        if self.waiter_count.load(Ordering::Relaxed) == 0 {
            return;
        }
        use crate::debug::htrace;
        htrace::enter(htrace::id::WAKE_ALL, self.waiter_count.load(Ordering::Relaxed) as u32);
        // Collect waiters while holding the lock, then resume AFTER releasing.
        // Calling resume() inside the lock acquires SCHEDULER with interrupts
        // disabled (SpinLock cli). With many poll waiters (XFCE has 20+),
        // the lock is held for >10ms — longer than the timer period. This
        // prevents the timer handler from returning, starving ALL timers.
        let waiters: alloc::vec::Vec<Arc<Process>> = {
            let mut queue = self.queue.lock();
            let mut v = alloc::vec::Vec::new();
            while let Some(process) = queue.pop_front() {
                self.waiter_count.fetch_sub(1, Ordering::Relaxed);
                v.push(process);
            }
            v
        }; // queue lock released
        let woken_count = waiters.len() as u32;
        // Guard: skip any Arc<Process> whose inner pointer is outside the
        // kernel direct-map range. Those came from a heap-corrupted
        // VecDeque buffer (bad Arc would crash inside resume() and again
        // when Vec<Arc> is dropped). Drain so we have ownership and can
        // forget the bad ones instead of dropping them.
        //
        // Arch-specific kernel base: x86_64 uses 0xffff_8000_0000_0000 (high
        // half starts at bit 47), ARM64 uses 0xffff_0000_0000_0000 (high half
        // starts at bit 48).  Hardcoding the x86 value previously caused every
        // valid arm64 kernel Arc to be treated as corrupt — silently dropping
        // every wait-queue wakeup.  Use the platform's KERNEL_BASE_ADDR.
        let mut warned = false;
        let trace = crate::fs::epoll::EPOLL_TRACE_FD.load(Ordering::Relaxed) != 0;
        let mut woken_pids: alloc::vec::Vec<i32> = alloc::vec::Vec::new();
        for process in waiters.into_iter() {
            let raw = Arc::as_ptr(&process) as usize;
            if raw < kevlar_platform::arch::KERNEL_BASE_ADDR {
                if !warned {
                    log::warn!(
                        "WAITQ CORRUPT: queue={:p} bad arc={:#x} — forgetting",
                        self, raw,
                    );
                    warned = true;
                }
                core::mem::forget(process);
                continue;
            }
            if trace {
                woken_pids.push(process.pid().as_i32());
            }
            process.resume();
        }
        if trace && woken_count > 0 {
            log::info!("WAITQ wake_all queue={:p} woken={} pids={:?}",
                self, woken_count, woken_pids);
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
        let waiters: alloc::vec::Vec<Arc<Process>> = {
            let mut queue = self.queue.lock();
            let mut v = alloc::vec::Vec::new();
            while v.len() < max as usize {
                if let Some(process) = queue.pop_front() {
                    self.waiter_count.fetch_sub(1, Ordering::Relaxed);
                    v.push(process);
                } else {
                    break;
                }
            }
            v
        };
        let woken = waiters.len() as u32;
        for process in &waiters {
            process.resume();
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
