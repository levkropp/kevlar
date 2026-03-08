// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::process::current_process;
use crate::result::Result;
use crate::syscalls::SyscallHandler;

impl<'a> SyscallHandler<'a> {
    pub fn sys_umask(&mut self, new_mask: u32) -> Result<isize> {
        let old = current_process().set_umask(new_mask);
        Ok(old as isize)
    }
}
