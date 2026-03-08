// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::process::{current_process, Process};
use crate::result::Result;
use crate::syscalls::SyscallHandler;

impl<'a> SyscallHandler<'a> {
    pub fn sys_vfork(&mut self) -> Result<isize> {
        // For now, vfork behaves identically to fork.
        // A proper vfork would suspend the parent until the child calls exec or _exit,
        // but fork semantics are safe and correct.
        Process::fork(current_process(), self.frame).map(|child| child.pid().as_i32() as isize)
    }
}
