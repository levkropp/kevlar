// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! vfork(2): create child sharing address space, parent suspended.
//!
//! The child runs inline — no context switch. The parent's kernel stack
//! is frozen at the vfork syscall return point. When the child calls
//! _exit() or exec(), control returns to the parent.
use crate::process::{current_process, switch, Process, ProcessState, VFORK_WAIT_QUEUE};
use crate::result::Result;
use crate::syscalls::SyscallHandler;

impl<'a> SyscallHandler<'a> {
    pub fn sys_vfork(&mut self) -> Result<isize> {
        let child = Process::vfork(current_process(), self.frame)?;
        let child_pid = child.pid().as_i32() as isize;

        // Block parent until child exits or execs.
        VFORK_WAIT_QUEUE.sleep_signalable_until(|| {
            match child.state() {
                ProcessState::ExitedWith(_) => Ok(Some(())),
                _ => Ok(None),
            }
        })?;

        Ok(child_pid)
    }
}
