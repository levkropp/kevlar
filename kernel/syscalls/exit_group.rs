// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::{ctypes::*, process::Process, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_exit_group(&mut self, status: c_int) -> ! {
        // TODO: Kill other threads belonging to the same thread group.
        Process::exit_group(status);
    }
}
