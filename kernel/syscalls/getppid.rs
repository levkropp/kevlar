// SPDX-License-Identifier: MIT OR Apache-2.0
use crate::{process::current_process, result::Result, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_getppid(&mut self) -> Result<isize> {
        Ok(current_process().ppid().as_i32() as isize)
    }
}
