// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
// Own implementation based on Linux man pages.
use crate::fs::path::Path;
use crate::prelude::*;
use crate::{process::current_process, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_access(&mut self, path: &Path) -> Result<isize> {
        // Resolve the path — if it doesn't exist, lookup returns ENOENT.
        // We don't check real permissions since we run as root (uid 0).
        let _inode = current_process().root_fs().lock_no_irq().lookup(path)?;
        Ok(0)
    }
}
