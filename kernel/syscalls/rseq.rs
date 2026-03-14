// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::result::{Errno, Result};
use crate::syscalls::SyscallHandler;

impl<'a> SyscallHandler<'a> {
    /// rseq(2) — restartable sequences. glibc 2.35+ calls this during init.
    /// Return ENOSYS so glibc falls back to non-rseq paths.
    pub fn sys_rseq(&mut self, _rseq: usize, _len: u32, _flags: i32, _sig: u32) -> Result<isize> {
        Err(Errno::ENOSYS.into())
    }
}
