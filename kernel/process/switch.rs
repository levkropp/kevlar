// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use super::*;
use crate::process::PId;
use crate::process::process::PROCESSES;
use crate::arch::{self};

use alloc::sync::Arc;

use core::mem::{self};
use kevlar_platform::spinlock::SpinLock;

/// finish_task_switch pickup slot.  When a task exits and calls switch(),
/// we can't release its kernel stacks from the exiting context — the
/// assembly switch hasn't happened yet and the code after `switch_thread`
/// runs on the incoming task's frame, not the outgoing one.  Stash the
/// exiting Arc here; the *next* task to run on this CPU cleans it up.
///
/// Single-CPU for now — extend to per-CPU array when SMP lands.
static PREV_EXITED: SpinLock<Option<Arc<Process>>> = SpinLock::new(None);

/// Yields execution to another thread.
///
/// Returns `true` if we actually switched to a different thread, `false` if
/// we kept running the current thread (no other runnable threads were found).
pub fn switch() -> bool {
    let _switch_span = crate::debug::tracer::span_guard(
        crate::debug::tracer::span::CTX_SWITCH);
    // Prevent the per-CPU timer preemption handler from calling switch()
    // re-entrantly while we are already in the middle of a context switch.
    // Without this, a timer IRQ could nest switch() on the same CPU, causing
    // do_switch_thread to save a mid-switch RSP for the outgoing thread and
    // ultimately corrupting its kernel stack.
    kevlar_platform::arch::preempt_disable();

    let prev = current_process().clone();
    let prev_pid = prev.pid();
    let prev_state = prev.state();
    // For Runnable (preempted) threads: mark context_saved = false BEFORE
    // re-enqueueing.  do_switch_thread will save the new RSP and reset
    // context_saved = true.  This prevents another CPU from loading the
    // stale (pre-preemption) RSP between the enqueue and the save.
    if prev_pid != PId::new(0) && prev_state == ProcessState::Runnable {
        prev.arch().context_saved.store(false, core::sync::atomic::Ordering::Release);
    }

    // ── Phase 1: scheduler decision (SCHEDULER lock only) ───────────────
    // Release SCHEDULER before acquiring PROCESSES to avoid the
    // SCHEDULER→PROCESSES nested lock.  The window between releasing
    // SCHEDULER and locking PROCESSES is safe: if the chosen PID is
    // removed by exit_group() in that window, the PROCESSES lookup
    // returns None and we fall back to the idle thread.
    let next_pid = {
        let scheduler = SCHEDULER.lock();

        // Push back the currently running thread to the runqueue if it's still
        // ready for running, in other words, it's not blocked.
        if prev_pid != PId::new(0) && prev_state == ProcessState::Runnable {
            scheduler.enqueue(prev_pid);
        }

        // Pick a thread to run next.
        scheduler.pick_next()
    };

    // ── Phase 2: resolve PID to Arc<Process> (PROCESSES lock only) ────
    let next = match next_pid {
        Some(next_pid) => {
            // Fast path: if we picked ourselves, reuse the existing Arc
            // instead of locking PROCESSES for a hash lookup.
            if next_pid == prev_pid {
                // Self-yield: skip the full context switch entirely.
                prev.arch().context_saved.store(true, core::sync::atomic::Ordering::Release);
                kevlar_platform::arch::preempt_enable();
                return false;
            }
            // Defensive None-check: exit_group() removes a sibling from the
            // scheduler queues before removing it from PROCESSES, but there is
            // still a narrow window where pick_next() returns a PID that is
            // already gone.  Fall back to the idle thread rather than panic.
            match PROCESSES.lock().get(&next_pid) {
                Some(p) if p.state() == ProcessState::Runnable => p.clone(),
                _ => IDLE_THREAD.get().get().clone(),
            }
        }
        None => IDLE_THREAD.get().get().clone(),
    };

    if Arc::ptr_eq(&prev, &next) {
        // Continue executing the current process.
        // Restore context_saved = true since we're not actually switching.
        prev.arch().context_saved.store(true, core::sync::atomic::Ordering::Release);
        kevlar_platform::arch::preempt_enable();
        return false;
    }

    // Re-check next's state after releasing SCHEDULER.lock(): exit_group() can
    // mark a just-dequeued Runnable thread as ExitedWith between pick_next()
    // and here.  Fall back to idle rather than switching into an exiting thread.
    // (The original debug_assert! was too strong and would panic on this race.)
    let next = if next.state() != ProcessState::Runnable {
        drop(next);
        IDLE_THREAD.get().get().clone()
    } else {
        next
    };

    // After a possible fallback to idle re-check whether we are back on the
    // same thread (i.e. prev IS the idle thread).
    if Arc::ptr_eq(&prev, &next) {
        prev.arch().context_saved.store(true, core::sync::atomic::Ordering::Release);
        kevlar_platform::arch::preempt_enable();
        return false;
    }

    // Spinwait until next's kernel context (RSP) is fully saved.  This can
    // only be false if next is a Runnable thread that was preempted on another
    // CPU and has not yet reached the do_switch_thread save point.  The window
    // is very small (a few instructions), but real on SMP.  Safe to spin here
    // because preempt_count > 0 means the timer won't re-enter switch() on
    // this CPU, and the CPU saving next's RSP does so in assembly without
    // acquiring any lock.
    while !next.arch().context_saved.load(core::sync::atomic::Ordering::Acquire) {
        core::hint::spin_loop();
    }

    if let Some(vm) = next.vm().clone() {
        let lock = vm.lock_no_irq();
        let _pt_span = crate::debug::tracer::span_guard(
            crate::debug::tracer::span::PT_SWITCH);
        lock.page_table().switch();
    } else {
        // Task has no Vm (idle thread, kernel thread).  Load the kernel
        // bootstrap PML4 so CR3 doesn't keep pointing at the outgoing
        // task's pml4.  Otherwise, when that task's Vm is later torn
        // down, the hardware walker on this CPU can still traverse the
        // freed PT pages via stale CR3, writing A/D bits into pool-cached
        // pages and corrupting the PT_PAGE_MAGIC cookie.
        kevlar_platform::arch::load_kernel_page_table();
    }

    kevlar_platform::sync::arc_leak_one_ref(&next);

    kevlar_platform::flight_recorder::record(
        kevlar_platform::flight_recorder::kind::CTX_SWITCH,
        prev_pid.as_i32() as u32,
        next.pid().as_i32() as u64,
        0,
    );

    #[cfg(feature = "ktrace-sched")]
    crate::debug::ktrace::trace(crate::debug::ktrace::event::CTX_SWITCH,
        prev_pid.as_i32() as u32, next.pid().as_i32() as u32, 0, 0, 0);

    // Task #25 diagnostic: track when PID 1 was last scheduled.
    // The timer ISR uses this to detect and dump PID 1 starvation.
    if next.pid().as_i32() == 1 {
        let t = crate::timer::monotonic_ticks();
        let prev_last = crate::timer::PID1_LAST_TICK
            .swap(t, core::sync::atomic::Ordering::Relaxed);
        // Re-arm the stall dump so the next stall event prints again.
        crate::timer::pid1_stall_rearm();
        // First-seen print: confirm the detector is wired.
        if prev_last == 0 {
            warn!("PID1_TRACKER: first observation at tick={}", t);
        }
    }

    CURRENT.as_mut().set(next.clone());
    let prev_exited = matches!(prev_state,
        ProcessState::ExitedWith(_) | ProcessState::ExitedBySignal(_));

    // Before switching, finish any *previous* switch's pending cleanup:
    // the outgoing task on the last switch may have parked its Arc in
    // PREV_EXITED for us to reap now that we're off its stack.  This
    // mirrors Linux's finish_task_switch — cleanup runs in the incoming
    // task's context after the switch-in has already happened, so
    // releasing the outgoing task's kernel stacks is safe.
    if let Some(earlier_exited) = PREV_EXITED.lock().take() {
        #[allow(unsafe_code)]
        unsafe { earlier_exited.arch().release_stacks(); }
        // Drop the Arc explicitly outside the lock.
        drop(earlier_exited);
    }

    // Stash the currently-exiting prev for the next task on this CPU to
    // clean up (see finish_task_switch above).  Single-CPU-only wiring;
    // extend to per-CPU array when SMP lands.
    if prev_exited {
        let mut slot = PREV_EXITED.lock();
        // If the previous exiting task is still pending (nobody ran
        // between two exits), release it now — we'll stash ourselves next.
        if let Some(stale) = slot.take() {
            #[allow(unsafe_code)]
            unsafe { stale.arch().release_stacks(); }
            drop(stale);
        }
        *slot = Some(prev.clone());
    }

    {
        let _dst = crate::debug::tracer::span_guard(
            crate::debug::tracer::span::DO_SWITCH_THREAD);
        arch::switch_thread(prev.arch(), next.arch());
    }

    // We are now executing on the next thread's kernel stack.
    // Re-enable preemption so the timer can preempt this thread normally.
    kevlar_platform::arch::preempt_enable();

    // Drop the `prev` clone here (decrements strong count by 1, mirroring the
    // clone() at the top of this function).  The exiting thread remains alive
    // via EXITED_PROCESSES; a non-exiting thread remains alive via PROCESSES.
    // mem::forget(next) because we already cancelled its extra count via
    // arc_leak_one_ref above.
    drop(prev);
    mem::forget(next);
    true
}
