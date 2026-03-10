// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::result::Result;
use crate::{process::current_process, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_getpid(&mut self) -> Result<isize> {
        // For threads, getpid() returns the thread group ID (TGID),
        // which is the PID of the thread group leader.
        Ok(current_process().tgid().as_i32() as isize)
    }
}
