// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::result::{Errno, Result};
use crate::syscalls::SyscallHandler;

/// Expected size of struct rseq (Linux 4.18+): 32 bytes.
const RSEQ_SIZE: u32 = 32;

impl<'a> SyscallHandler<'a> {
    /// rseq(2) — restartable sequences. glibc 2.35+ calls this during init.
    /// Match Linux's argument validation: reject invalid len/rseq with EINVAL
    /// before returning ENOSYS for valid-looking registration attempts.
    pub fn sys_rseq(&mut self, rseq: usize, len: u32, _flags: i32, _sig: u32) -> Result<isize> {
        if rseq == 0 || len < RSEQ_SIZE {
            return Err(Errno::EINVAL.into());
        }
        Err(Errno::ENOSYS.into())
    }
}
