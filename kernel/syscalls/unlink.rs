// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Reference: OSv fs/vfs/vfs_syscalls.cc (BSD-3-Clause) — sys_unlink.
// Resolves the parent directory, then calls dir.unlink(basename).
use crate::fs::path::Path;
use crate::prelude::*;
use crate::{process::current_process, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_unlink(&mut self, path: &Path) -> Result<isize> {
        let (parent, name) = path
            .parent_and_basename()
            .ok_or_else::<Error, _>(|| Errno::ENOENT.into())?;

        let root_fs = current_process().root_fs().lock();
        let parent_dir = root_fs.lookup_dir(parent)?;
        parent_dir.unlink(name)?;
        Ok(0)
    }
}
