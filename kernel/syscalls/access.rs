// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
// Own implementation based on Linux man pages.
use crate::fs::path::Path;
use crate::fs::permission::check_access;
use crate::prelude::*;
use crate::{process::current_process, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_access(&mut self, path: &Path, mode: u32) -> Result<isize> {
        let current = current_process();
        let root_fs = current.root_fs();
        let inode = root_fs.lock_no_irq().lookup(path)?;
        let stat = inode.stat()?;
        // access(2) uses real UID/GID, not effective.
        check_access(&stat, current.uid(), current.gid(), mode)?;
        Ok(0)
    }
}
