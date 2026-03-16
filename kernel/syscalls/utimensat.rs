// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (Linux utimensat(2) man page).
use crate::prelude::*;
use crate::process::current_process;
use crate::syscalls::{CwdOrFd, SyscallHandler};
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    pub fn sys_utimensat(
        &mut self,
        _dirfd: CwdOrFd,
        pathname: usize,
        _times: Option<UserVAddr>,
        _flags: i32,
    ) -> Result<isize> {
        // Verify the file exists — return ENOENT if not, so callers like
        // touch(1) know to create the file with open(O_CREAT) instead.
        if pathname != 0 {
            if let Ok(uaddr) = UserVAddr::new_nonnull(pathname) {
                let user_cstr = super::UserCStr::new(uaddr, 512)?;
                let path = crate::fs::path::Path::new(user_cstr.as_str());
                let root_fs = current_process().root_fs();
                root_fs.lock().lookup(&path)?;
            }
        }
        // tmpfs timestamps are not persistent, so we just accept success.
        Ok(0)
    }
}
