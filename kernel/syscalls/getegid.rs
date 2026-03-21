// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::process::current_process;
use crate::result::Result;
use crate::syscalls::SyscallHandler;

impl<'a> SyscallHandler<'a> {
    #[allow(dead_code)]
    pub fn sys_getegid(&mut self) -> Result<isize> {
        Ok(current_process().egid() as isize)
    }
}
