// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX chroot(2) man page).
use crate::fs::path::Path;
use crate::prelude::*;
use crate::process::current_process;
use crate::syscalls::SyscallHandler;

impl<'a> SyscallHandler<'a> {
    pub fn sys_chroot(&mut self, path: &Path) -> Result<isize> {
        let current = current_process();

        // Give this process its own RootFs so the chroot doesn't affect
        // the parent (root_fs is Arc-shared after fork).
        current.unshare_root_fs();

        let root_fs = current.root_fs();
        let mut root_fs = root_fs.lock();
        root_fs.chroot(path)?;
        Ok(0)
    }
}
