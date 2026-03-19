// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::{prelude::*, process::current_process};
use crate::{process::Process, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_rt_sigreturn(&mut self) -> Result<isize> {
        let current = current_process();
        Process::restore_signaled_user_stack(&current, self.frame);

        // If we came from sigsuspend, restore the original signal mask now
        // that the handler has run.
        current.sigsuspend_restore_mask();

        // Return the RAX value from the restored frame so the original
        // syscall's return value is preserved (not overwritten with -EINTR).
        Ok(self.frame.rax as isize)
    }
}
