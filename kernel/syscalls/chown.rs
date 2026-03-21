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

/// Resolve uid/gid: -1 (0xFFFFFFFF) means "keep current".
pub fn resolve_owner(inode: &kevlar_vfs::inode::INode, uid: u32, gid: u32) -> Result<(UId, GId)> {
    let st = inode.stat()?;
    let new_uid = if uid == 0xFFFFFFFF { st.uid } else { UId::new(uid) };
    let new_gid = if gid == 0xFFFFFFFF { st.gid } else { GId::new(gid) };
    Ok((new_uid, new_gid))
}

impl<'a> SyscallHandler<'a> {
    pub fn sys_chown(&mut self, path: &Path, uid: u32, gid: u32) -> Result<isize> {
        let root_fs = current_process().root_fs();
        let root_fs = root_fs.lock();
        let inode = root_fs.lookup_inode(path, true)?;
        let (new_uid, new_gid) = resolve_owner(&inode, uid, gid)?;
        inode.chown(new_uid, new_gid)?;
        Ok(0)
    }

    pub fn sys_fchown(&mut self, fd: i32, uid: u32, gid: u32) -> Result<isize> {
        let current = current_process();
        let opened_files = current.opened_files().lock();
        let file = opened_files.get(Fd::new(fd))?;
        let inode = file.inode();
        let (new_uid, new_gid) = resolve_owner(&inode, uid, gid)?;
        inode.chown(new_uid, new_gid)?;
        Ok(0)
    }
}
