// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::result::{Errno, Result};
use crate::syscalls::SyscallHandler;

/// Minimum size of clone_args (v0): 64 bytes on Linux.
const CLONE_ARGS_SIZE_VER0: usize = 64;

impl<'a> SyscallHandler<'a> {
    /// clone3(2) — glibc 2.34+ tries clone3 first, falls back to clone on ENOSYS.
    /// Match Linux's argument validation: reject invalid size with EINVAL before
    /// returning ENOSYS for valid-looking calls.
    pub fn sys_clone3(&mut self, _cl_args: usize, size: usize) -> Result<isize> {
        if size < CLONE_ARGS_SIZE_VER0 {
            return Err(Errno::EINVAL.into());
        }
        Err(Errno::ENOSYS.into())
    }
}
