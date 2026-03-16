// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX unlinkat(2) man page).
use super::CwdOrFd;
use crate::fs::path::Path;
use crate::prelude::*;
use crate::{process::current_process, syscalls::SyscallHandler};

const AT_REMOVEDIR: i32 = 0x200;

impl<'a> SyscallHandler<'a> {
    pub fn sys_unlinkat(&mut self, dirfd: CwdOrFd, path: &Path, flags: i32) -> Result<isize> {
        let current = current_process();
        let root_fs_arc = current.root_fs();
        let root_fs = root_fs_arc.lock();
        let opened_files = current.opened_files().lock();

        let (parent_path, name) = root_fs.lookup_parent_path_at(
            &opened_files, &dirfd, path, true,
        )?;
        let parent_dir = parent_path.inode.as_dir()?;

        if flags & AT_REMOVEDIR != 0 {
            parent_dir.rmdir(name)?;
        } else {
            parent_dir.unlink(name)?;
        }
        Ok(0)
    }
}
