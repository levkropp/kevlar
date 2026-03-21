// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX mkdirat(2) man page).
use super::CwdOrFd;
use crate::fs::{path::Path, stat::FileMode};
use crate::prelude::*;
use crate::{process::current_process, syscalls::SyscallHandler};
use kevlar_vfs::stat::{GId, UId};

impl<'a> SyscallHandler<'a> {
    pub fn sys_mkdirat(&mut self, dirfd: CwdOrFd, path: &Path, mode: FileMode) -> Result<isize> {
        let current = current_process();
        let root_fs_arc = current.root_fs();
        let root_fs = root_fs_arc.lock();
        let opened_files = current.opened_files().lock();

        let (parent_inode, name) = root_fs.lookup_parent_inode_at(
            &opened_files, &dirfd, path, true,
        )?;
        parent_inode.as_dir()?.create_dir(name, mode, UId::new(current.euid()), GId::new(current.egid()))?;
        Ok(0)
    }
}
