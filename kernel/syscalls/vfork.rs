// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! vfork(2): create child sharing address space, parent suspended.
//!
//! The child runs inline — no context switch. The parent's kernel stack
//! is frozen at the vfork syscall return point. When the child calls
//! _exit() or exec(), control returns to the parent.
use crate::process::{current_process, signal::SigSet, Process, VFORK_WAIT_QUEUE};
use crate::result::Result;
use crate::syscalls::SyscallHandler;
use core::sync::atomic::Ordering;

impl<'a> SyscallHandler<'a> {
    pub fn sys_vfork(&mut self) -> Result<isize> {
        let child = Process::vfork(current_process(), self.frame)?;
        let child_pid = child.pid().as_i32() as isize;
        let current = current_process();

        // Block all signals for the duration of the vfork wait. Signals
        // remain queued and are delivered when this syscall returns.
        // Prevents EINTR spin from pending signals (e.g. SIGALRM).
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
        Ok(child_pid)
    }
}
