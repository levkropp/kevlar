// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Reference: OSv fs/vfs/vfs_syscalls.cc (BSD-3-Clause) — sys_rename.
// Resolves both parent directories, then calls old_parent.rename().
use crate::fs::path::Path;
use crate::prelude::*;
use crate::{process::current_process, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_rename(&mut self, old_path: &Path, new_path: &Path) -> Result<isize> {
        let (old_parent, old_name) = old_path
            .parent_and_basename()
            .ok_or_else::<Error, _>(|| Errno::ENOENT.into())?;
        let (new_parent, new_name) = new_path
            .parent_and_basename()
            .ok_or_else::<Error, _>(|| Errno::ENOENT.into())?;

        let root_fs = current_process().root_fs().lock();
        let old_parent_dir = root_fs.lookup_dir(old_parent)?;
        let new_parent_dir = root_fs.lookup_dir(new_parent)?;
        old_parent_dir.rename(old_name, &new_parent_dir, new_name)?;
        Ok(0)
    }
}
