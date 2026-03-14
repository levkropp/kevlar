// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::result::{Errno, Result};
use crate::syscalls::SyscallHandler;

impl<'a> SyscallHandler<'a> {
    /// clone3(2) — glibc 2.34+ tries clone3 first, falls back to clone on ENOSYS.
    pub fn sys_clone3(&mut self, _cl_args: usize, _size: usize) -> Result<isize> {
        Err(Errno::ENOSYS.into())
    }
}
