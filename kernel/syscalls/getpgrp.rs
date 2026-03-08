// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::process::current_process;
use crate::result::Result;
use crate::syscalls::SyscallHandler;

impl<'a> SyscallHandler<'a> {
    pub fn sys_getpgrp(&mut self) -> Result<isize> {
        // getpgrp() is equivalent to getpgid(0).
        let pgid = current_process().process_group().lock().pgid();
        Ok(pgid.as_i32() as isize)
    }
}
