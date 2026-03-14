// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! flock(2) — advisory file locking (stub).
use crate::{result::Result, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_flock(&mut self, _fd: i32, _operation: i32) -> Result<isize> {
        Ok(0)
    }
}
