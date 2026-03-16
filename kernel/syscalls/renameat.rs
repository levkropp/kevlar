// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX renameat(2) man page).
use super::CwdOrFd;
use crate::fs::path::Path;
use crate::prelude::*;
use crate::{process::current_process, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_renameat(
        &mut self,
        olddirfd: CwdOrFd,
        oldpath: &Path,
        newdirfd: CwdOrFd,
        newpath: &Path,
    ) -> Result<isize> {
        let current = current_process();
        let root_fs_arc = current.root_fs();
        let root_fs = root_fs_arc.lock();
        let opened_files = current.opened_files().lock();

        let (old_parent, old_name) = root_fs.lookup_parent_path_at(
            &opened_files, &olddirfd, oldpath, true,
        )?;
        let (new_parent, new_name) = root_fs.lookup_parent_path_at(
            &opened_files, &newdirfd, newpath, true,
        )?;

        let old_dir = old_parent.inode.as_dir()?;
        let new_dir = new_parent.inode.as_dir()?;
        old_dir.rename(old_name, new_dir, new_name)?;
        Ok(0)
    }
}
