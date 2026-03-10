// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (Linux utimensat(2) man page).
use crate::prelude::*;
use crate::syscalls::{CwdOrFd, SyscallHandler};
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    pub fn sys_utimensat(
        &mut self,
        _dirfd: CwdOrFd,
        _pathname: usize,
        _times: Option<UserVAddr>,
        _flags: i32,
    ) -> Result<isize> {
        // Stub: accept and return success.
        // tmpfs timestamps are not persistent, so silently accepting is correct.
        // A real implementation would update inode atime/mtime.
        Ok(0)
    }
}
