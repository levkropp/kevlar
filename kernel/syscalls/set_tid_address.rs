// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::process::current_process;
use crate::result::Result;
use crate::syscalls::SyscallHandler;
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    pub fn sys_set_tid_address(&mut self, uaddr: UserVAddr) -> Result<isize> {
        current_process().set_clear_child_tid(uaddr.value());
        Ok(current_process().pid().as_i32() as isize)
    }
}
