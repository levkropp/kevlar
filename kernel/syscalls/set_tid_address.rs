// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::process::current_process;
use crate::result::Result;
use crate::syscalls::SyscallHandler;
use kevlar_runtime::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    pub fn sys_set_tid_address(&mut self, _uaddr: UserVAddr) -> Result<isize> {
        // TODO: Store uaddr as clear_child_tid for futex wake on exit.
        Ok(current_process().pid().as_i32() as isize)
    }
}
