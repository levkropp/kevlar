// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::result::Result;
use crate::syscalls::SyscallHandler;

impl<'a> SyscallHandler<'a> {
    pub fn sys_set_robust_list(&mut self, _head: usize, _len: usize) -> Result<isize> {
        // Stub: accept and ignore. Needed by musl's thread init.
        Ok(0)
    }
}
