// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::result::Result;
use crate::syscalls::SyscallHandler;

impl<'a> SyscallHandler<'a> {
    pub fn sys_madvise(&mut self, _addr: usize, _len: usize, _advice: i32) -> Result<isize> {
        // Stub: accept all advice values silently.
        // TODO: Implement MADV_DONTNEED (free pages in range).
        Ok(0)
    }
}
