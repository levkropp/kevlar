// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX fchmod(2) man page).
// Stub — tmpfs doesn't track permission changes yet.
use crate::{prelude::*, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_fchmod(&mut self, _fd: i32, _mode: u32) -> Result<isize> {
        // Silently succeed — tmpfs ignores permission changes.
        Ok(0)
    }

    pub fn sys_fchmodat(&mut self, _dirfd: i32, _path: &crate::fs::path::PathBuf, _mode: u32, _flags: i32) -> Result<isize> {
        Ok(0)
    }

    pub fn sys_fchownat(&mut self, _dirfd: i32, _path: &crate::fs::path::PathBuf, _uid: u32, _gid: u32, _flags: i32) -> Result<isize> {
        Ok(0)
    }
}
