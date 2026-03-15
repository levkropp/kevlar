// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! mknod(2) / mknodat(2) — create device special files.
use crate::{
    fs::devfs::DeviceNodeFile,
    prelude::*,
    process::current_process,
    syscalls::SyscallHandler,
};
use kevlar_vfs::stat::{S_IFBLK, S_IFCHR, S_IFMT};

impl<'a> SyscallHandler<'a> {
    pub fn sys_mknod(&mut self, path: &crate::fs::path::Path, mode: u32, dev: u32) -> Result<isize> {
        let file_type = mode & S_IFMT;

        // Only support character and block devices.
        if file_type != S_IFCHR && file_type != S_IFBLK {
            // Regular files / FIFOs: stub as success.
            return Ok(0);
        }

        let (parent_path, name) = path
            .parent_and_basename()
            .ok_or_else(|| crate::result::Error::new(Errno::ENOENT))?;

        let root_fs = current_process().root_fs();
        let root_fs = root_fs.lock();
        let parent_dir = root_fs.lookup_dir(parent_path)?;

        // Create a DeviceNodeFile and insert it into the parent directory.
        let node = Arc::new(DeviceNodeFile::new(mode, dev)) as Arc<dyn crate::fs::inode::FileLike>;
        parent_dir.link(name, &kevlar_vfs::inode::INode::FileLike(node))?;

        Ok(0)
    }
}
