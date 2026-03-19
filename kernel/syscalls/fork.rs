// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::{
    process::{current_process, signal::SigSet, GHOST_FORK_ENABLED, Process, VFORK_WAIT_QUEUE},
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
            let saved_mask = current.sigset_load();
            current.sigset_store(SigSet::ALL);

            while !child.ghost_fork_done.load(Ordering::Acquire) {
                let _ = VFORK_WAIT_QUEUE.sleep_signalable_until(|| {
                    if child.ghost_fork_done.load(Ordering::Acquire) {
                        Ok(Some(()))
                    } else {
                        Ok(None)
                    }
                });
            }

            current.sigset_store(saved_mask);
        }

        Ok(child_pid)
    }
}
