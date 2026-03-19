// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::result::Result;
use crate::syscalls::SyscallHandler;

impl<'a> SyscallHandler<'a> {
    pub fn sys_set_robust_list(&mut self, _head: usize, len: usize) -> Result<isize> {
        // Linux requires len == sizeof(struct robust_list_head) == 24 on x86-64.
        if len != 24 {
            return Err(crate::result::Errno::EINVAL.into());
        }
        // Stub: accept and ignore. Needed by musl's thread init.
        Ok(0)
    }
}
