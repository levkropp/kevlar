// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX chown(2), fchown(2) man pages).
use crate::{
    fs::{opened_file::Fd, path::Path},
    prelude::*,
    process::current_process,
    syscalls::SyscallHandler,
};
use kevlar_vfs::stat::{GId, UId};

impl<'a> SyscallHandler<'a> {
    pub fn sys_chown(&mut self, path: &Path, uid: u32, gid: u32) -> Result<isize> {
        let root_fs = current_process().root_fs();
        let root_fs = root_fs.lock();
        let inode = root_fs.lookup_inode(path, true)?;
        inode.chown(UId::new(uid), GId::new(gid))?;
        Ok(0)
    }

    pub fn sys_fchown(&mut self, fd: i32, uid: u32, gid: u32) -> Result<isize> {
        let current = current_process();
        let opened_files = current.opened_files().lock();
        let file = opened_files.get(Fd::new(fd))?;
        file.inode().chown(UId::new(uid), GId::new(gid))?;
        Ok(0)
    }
}
