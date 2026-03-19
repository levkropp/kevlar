// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
// Own implementation based on Linux man pages.
use super::CwdOrFd;
use crate::fs::stat::{O_RDWR, O_WRONLY};
use crate::fs::{inode::INode, opened_file::OpenFlags, path::Path, stat::FileMode};
use crate::prelude::*;
use crate::{process::current_process, syscalls::SyscallHandler};

fn create_file_at(
    cwd_or_fd: &CwdOrFd,
    path: &Path,
    flags: OpenFlags,
    mode: FileMode,
) -> Result<INode> {
    if flags.contains(OpenFlags::O_DIRECTORY) {
        return Err(Errno::EINVAL.into());
    }

    let (parent_dir, name) = path
        .parent_and_basename()
        .ok_or_else::<Error, _>(|| Errno::EEXIST.into())?;

    let current = current_process();
    let opened_files = current.opened_files_no_irq();
    let root_fs_arc = current.root_fs();
    let root_fs = root_fs_arc.lock_no_irq();
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

        // Reject writes to read-only mounts (MS_RDONLY).
        let access = mode.access_mode();
        if (flags.contains(OpenFlags::O_CREAT) || access == O_WRONLY || access == O_RDWR)
            && path.is_absolute()
            && crate::fs::mount::MountTable::is_readonly(path.as_str())
        {
            return Err(Error::new(Errno::EROFS));
        }

        if flags.contains(OpenFlags::O_CREAT) {
            match create_file_at(&dirfd, path, flags, mode) {
                Ok(_) => {}
                Err(err) if !flags.contains(OpenFlags::O_EXCL) && err.errno() == Errno::EEXIST => {}
                Err(err) => return Err(err),
            }
        }

        // Resolve the path. For absolute paths and CWD-relative paths, avoid
        // holding the opened_files lock during path resolution to prevent
        // deadlocks (e.g., /proc/self/fd/N needs the fd table during lookup).
        let path_comp = {
            let root_fs_arc = current.root_fs();
            let root_fs = root_fs_arc.lock_no_irq();
            if path.is_absolute() || matches!(dirfd, CwdOrFd::AtCwd) {
                root_fs.lookup_path(path, true)?
            } else {
                // fd-relative path: need opened_files for dirfd resolution.
                let opened_files = current.opened_files_no_irq();
                root_fs.lookup_path_at(&opened_files, &dirfd, path, true)?
            }
        };

        if flags.contains(OpenFlags::O_DIRECTORY) && !path_comp.inode.is_dir() {
            return Err(Error::new(Errno::ENOTDIR));
        }

        let access_mode = mode.access_mode();
        if path_comp.inode.is_dir() && (access_mode == O_WRONLY || access_mode == O_RDWR) {
            return Err(Error::new(Errno::EISDIR));
        }

        let mut opened_files = current.opened_files_no_irq();
        let fd = opened_files.open(path_comp, flags.into())?;

        // O_TRUNC: truncate the file if opened for writing.
        if flags.contains(OpenFlags::O_TRUNC) {
            let access = flags.bits() & 0o3;
            if access == O_WRONLY as i32 || access == O_RDWR as i32 {
                if let Ok(file) = opened_files.get(fd)?.as_file() {
                    let _ = file.truncate(0);
                }
            }
        }

        Ok(fd.as_usize() as isize)
    }
}
