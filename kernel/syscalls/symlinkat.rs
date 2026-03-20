// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX symlinkat(2) man page).
use super::CwdOrFd;
use crate::fs::path::Path;
use crate::prelude::*;
use crate::{process::current_process, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    /// symlinkat(target, newdirfd, linkpath)
    pub fn sys_symlinkat(
        &mut self,
        target: &Path,
        newdirfd: CwdOrFd,
        linkpath: &Path,
    ) -> Result<isize> {
        let current = current_process();
        let root_fs_arc = current.root_fs();
        let root_fs = root_fs_arc.lock();
        let opened_files = current.opened_files().lock();

        let (parent_inode, name) = root_fs.lookup_parent_inode_at(
            &opened_files, &newdirfd, linkpath, true,
        )?;
        parent_inode.as_dir()?.create_symlink(name, target.as_str())?;
        Ok(0)
    }

    /// symlink(target, linkpath) — old-style, delegates to symlinkat(target, AT_FDCWD, linkpath)
    pub fn sys_symlink(&mut self, target: &Path, linkpath: &Path) -> Result<isize> {
        self.sys_symlinkat(target, CwdOrFd::AtCwd, linkpath)
    }
}
