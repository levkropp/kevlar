// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Reference: OSv fs/vfs/vfs_syscalls.cc (BSD-3-Clause) — sys_open logic with
// dirfd-relative path resolution. Adapted for Kevlar's existing lookup_path_at.
use super::CwdOrFd;
use crate::fs::stat::{O_RDWR, O_WRONLY};
use crate::fs::{inode::INode, opened_file::OpenFlags, path::Path, stat::FileMode};
use crate::prelude::*;
use crate::{process::current_process, syscalls::SyscallHandler};

fn create_file_at(cwd_or_fd: &CwdOrFd, path: &Path, flags: OpenFlags, mode: FileMode) -> Result<INode> {
    if flags.contains(OpenFlags::O_DIRECTORY) {
        return Err(Errno::EINVAL.into());
    }

    let (parent_dir, name) = path
        .parent_and_basename()
        .ok_or_else::<Error, _>(|| Errno::EEXIST.into())?;

    let current = current_process();
    let opened_files = current.opened_files().lock();
    let root_fs = current.root_fs().lock();
    let parent_path = root_fs.lookup_path_at(&opened_files, cwd_or_fd, parent_dir, true)?;
    parent_path.inode.as_dir()?.create_file(name, mode)
}

impl<'a> SyscallHandler<'a> {
    pub fn sys_openat(
        &mut self,
        dirfd: CwdOrFd,
        path: &Path,
        flags: OpenFlags,
        mode: FileMode,
    ) -> Result<isize> {
        let current = current_process();

        if flags.contains(OpenFlags::O_CREAT) {
            match create_file_at(&dirfd, path, flags, mode) {
                Ok(_) => {}
                Err(err) if !flags.contains(OpenFlags::O_EXCL) && err.errno() == Errno::EEXIST => {}
                Err(err) => return Err(err),
            }
        }

        let root_fs = current.root_fs().lock();
        let mut opened_files = current.opened_files().lock();

        let path_comp = root_fs.lookup_path_at(&opened_files, &dirfd, path, true)?;
        if flags.contains(OpenFlags::O_DIRECTORY) && !path_comp.inode.is_dir() {
            return Err(Error::new(Errno::ENOTDIR));
        }

        let access_mode = mode.access_mode();
        if path_comp.inode.is_dir() && (access_mode == O_WRONLY || access_mode == O_RDWR) {
            return Err(Error::new(Errno::EISDIR));
        }

        let fd = opened_files.open(path_comp, flags.into())?;
        Ok(fd.as_usize() as isize)
    }
}
