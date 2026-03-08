// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::{prelude::*, process::current_process};
use crate::{process::Process, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_rt_sigreturn(&mut self) -> Result<isize> {
        Process::restore_signaled_user_stack(current_process(), self.frame);
        Err(Errno::EINTR.into())
    }
}
