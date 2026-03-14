// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use super::*;
use crate::process::PId;
use crate::{
    arch::{self},
    process::process::PROCESSES,
};

use alloc::sync::Arc;

use core::mem::{self};

/// Yields execution to another thread.
///
/// Returns `true` if we actually switched to a different thread, `false` if
/// we kept running the current thread (no other runnable threads were found).
pub fn switch() -> bool {
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

    let next = {
        let scheduler = SCHEDULER.lock();

        // Push back the currently running thread to the runqueue if it's still
        // ready for running, in other words, it's not blocked.
        if prev_pid != PId::new(0) && prev_state == ProcessState::Runnable {
            scheduler.enqueue(prev_pid);
        }

        // Pick a thread to run next.
        match scheduler.pick_next() {
            Some(next_pid) => {
                // Fast path: if we picked ourselves, reuse the existing Arc
                // instead of locking PROCESSES for a hash lookup.
                if next_pid == prev_pid {
                    prev.clone()
                } else {
                    // Defensive None-check: exit_group() removes a sibling from the
                    // scheduler queues before removing it from PROCESSES, but there is
                    // still a narrow window where pick_next() returns a PID that is
                    // already gone.  Fall back to the idle thread rather than panic.
                    match PROCESSES.lock().get(&next_pid) {
                        Some(p) if p.state() == ProcessState::Runnable => p.clone(),
                        _ => IDLE_THREAD.get().get().clone(),
                    }
                }
            }
            None => IDLE_THREAD.get().get().clone(),
        }
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
        // Use lock_no_irq: we only read page_table().pml4 (immutable after init).
        // lock() would call cli and spin — if this CPU is inside sys_munmap holding
        // the VM lock via lock_no_irq (IF=1), a timer would fire, switch() would
        // pick the same thread's sibling, then cli+spin here while holding IF=0
        // → TLB shootdown IPI never delivered → deadlock.
        let lock = vm.lock_no_irq();
        lock.page_table().switch();
    }

    // Cancel the +1 from `next.clone()` below without calling Drop.  We will
    // mem::forget(next) so the local variable's Drop never runs; the CURRENT
    // slot takes over ownership of that extra count.
    kevlar_platform::sync::arc_leak_one_ref(&next);

    // Record context switch in the per-CPU flight recorder before the switch.
    kevlar_platform::flight_recorder::record(
        kevlar_platform::flight_recorder::kind::CTX_SWITCH,
        prev_pid.as_i32() as u32,
        next.pid().as_i32() as u64,
        0,
    );

    // Switch into the next thread.  We deliberately do NOT arc_leak prev here:
    // the `prev` local clone keeps its strong reference alive across the entire
    // arch::switch_thread call.  Without this, another CPU running
    // gc_exited_processes() could free the exiting thread's kernel stack while
    // do_switch_thread is still saving/loading the RSP from it, resulting in a
    // use-after-free (observed as rip=0 / null function-pointer crash).
    // After arch::switch_thread returns we are on next's kernel stack and it
    // is safe to drop `prev`.
    CURRENT.as_mut().set(next.clone());
    arch::switch_thread(prev.arch(), next.arch());

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
