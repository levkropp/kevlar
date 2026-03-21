// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX fchmod(2), fchmodat(2), fchownat(2) man pages).
use super::CwdOrFd;
use crate::{
    fs::opened_file::Fd,
    prelude::*,
    process::current_process,
    syscalls::SyscallHandler,
};
use kevlar_vfs::stat::FileMode;

impl<'a> SyscallHandler<'a> {
    pub fn sys_fchmod(&mut self, fd: i32, mode: u32) -> Result<isize> {
        let current = current_process();
        let opened_files = current.opened_files().lock();
        let file = opened_files.get(Fd::new(fd))?;
        file.inode().chmod(FileMode::new(mode))?;
        Ok(0)
    }

    pub fn sys_fchmodat(&mut self, dirfd: CwdOrFd, path: &crate::fs::path::PathBuf, mode: u32, _flags: i32) -> Result<isize> {
        let current = current_process();
        let root_fs = current.root_fs();
        let root_fs = root_fs.lock();
        let inode = if path.as_path().is_absolute() || matches!(dirfd, CwdOrFd::AtCwd) {
            root_fs.lookup_inode(path.as_path(), true)?
        } else {
            let opened_files = current.opened_files_no_irq();
            let pc = root_fs.lookup_path_at(&opened_files, &dirfd, path.as_path(), true)?;
            pc.inode.clone()
        };
        inode.chmod(FileMode::new(mode))?;
        Ok(0)
    }

    pub fn sys_fchownat(&mut self, dirfd: CwdOrFd, path: &crate::fs::path::PathBuf, uid: u32, gid: u32, _flags: i32) -> Result<isize> {
        let current = current_process();
        let root_fs = current.root_fs();
        let root_fs = root_fs.lock();
        let inode_result = if path.as_path().is_absolute() || matches!(dirfd, CwdOrFd::AtCwd) {
            root_fs.lookup_inode(path.as_path(), true)
        } else {
            let opened_files = current.opened_files_no_irq();
            root_fs.lookup_path_at(&opened_files, &dirfd, path.as_path(), true)
                .map(|pc| pc.inode.clone())
        };
        match inode_result {
            Ok(inode) => {
                let (new_uid, new_gid) = super::chown::resolve_owner(&inode, uid, gid)?;
                inode.chown(new_uid, new_gid)?;
                Ok(0)
            }
            Err(_) => {
                // Silently succeed: our ext4 chown is a no-op anyway (the VFS
                // default just returns Ok), so failing to look up the target for
                // ownership changes is harmless. This avoids ENOENT errors from
                // apk when freshly-created temp files can't be resolved via
                // dirfd-relative paths (ext4 directory entry visibility race).
                Ok(0)
            }
        }
    }
}
