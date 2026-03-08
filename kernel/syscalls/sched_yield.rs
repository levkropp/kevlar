// SPDX-License-Identifier: MIT OR Apache-2.0
use crate::process::switch;
use crate::result::Result;
use crate::syscalls::SyscallHandler;

impl<'a> SyscallHandler<'a> {
    pub fn sys_sched_yield(&mut self) -> Result<isize> {
        switch();
        Ok(0)
    }
}
