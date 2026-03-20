// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::{
    process::{current_process, signal::SigSet, GHOST_FORK_ENABLED, Process},
    result::Result,
    syscalls::SyscallHandler,
};
use core::sync::atomic::Ordering;

impl<'a> SyscallHandler<'a> {
    pub fn sys_fork(&mut self) -> Result<isize> {
        let child = Process::fork(current_process(), self.frame)?;
        let child_pid = child.pid().as_i32() as isize;

        // Ghost-fork: parent blocked until child exec's or exits.
        // The child shares the parent's VM (no page table duplication).
        // This is safe because the parent is asleep while the child uses
        // the shared address space; exec replaces it with a new VM.
        //
        // Block all signals for the duration of the wait. Signals remain
        // queued (signal_pending bits stay set) and are delivered when this
        // syscall returns via try_delivering_signal. This prevents a spin
        // where a pending signal (e.g. SIGALRM) causes sleep_signalable_until
        // to return EINTR on every iteration without ever actually sleeping.
        if GHOST_FORK_ENABLED.load(Ordering::Relaxed) {
            let current = current_process();
            // Block all signals during ghost-fork wait. Signals remain queued
            // and are delivered when fork() returns (via try_delivering_signal).
            let saved_mask = current.sigset_load();
            current.sigset_store(SigSet::ALL);

            // Spin-wait for ghost child to exec/exit. On SMP (4 CPUs), the
            // child runs on another CPU so spinning doesn't block it.
            // Yield periodically for single-CPU fallback safety.
            while !child.ghost_fork_done.load(Ordering::Acquire) {
                for _ in 0..256 {
                    core::hint::spin_loop();
                    if child.ghost_fork_done.load(Ordering::Acquire) {
                        break;
                    }
                }
                if !child.ghost_fork_done.load(Ordering::Acquire) {
                    crate::process::switch();
                }
            }

            current.sigset_store(saved_mask);
            // Flush TLB to pick up restored writable PTEs from the ghost child.
            if let Some(vm_ref) = current.vm().as_ref() {
                vm_ref.lock().page_table().flush_tlb_all();
            }
        }

        Ok(child_pid)
    }
}
