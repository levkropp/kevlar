// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX fchmod(2), fchmodat(2), fchownat(2) man pages).
use crate::{
    fs::opened_file::Fd,
    prelude::*,
    process::current_process,
    syscalls::SyscallHandler,
};
use kevlar_vfs::stat::{FileMode, GId, UId};

impl<'a> SyscallHandler<'a> {
    pub fn sys_fchmod(&mut self, fd: i32, mode: u32) -> Result<isize> {
        let current = current_process();
        let opened_files = current.opened_files().lock();
        let file = opened_files.get(Fd::new(fd))?;
        file.inode().chmod(FileMode::new(mode))?;
        Ok(0)
    }

    pub fn sys_fchmodat(&mut self, _dirfd: i32, path: &crate::fs::path::PathBuf, mode: u32, _flags: i32) -> Result<isize> {
        let root_fs = current_process().root_fs();
        let root_fs = root_fs.lock();
        let inode = root_fs.lookup_inode(path.as_path(), true)?;
        inode.chmod(FileMode::new(mode))?;
        Ok(0)
    }

    pub fn sys_fchownat(&mut self, _dirfd: i32, path: &crate::fs::path::PathBuf, uid: u32, gid: u32, _flags: i32) -> Result<isize> {
        let root_fs = current_process().root_fs();
        let root_fs = root_fs.lock();
        let inode = root_fs.lookup_inode(path.as_path(), true)?;
        inode.chown(UId::new(uid), GId::new(gid))?;
        Ok(0)
    }
}
