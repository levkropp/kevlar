// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
// Own implementation based on Linux man pages.
use super::CwdOrFd;
use crate::fs::stat::{O_RDWR, O_WRONLY};
use crate::fs::{opened_file::OpenFlags, path::Path, stat::FileMode};
use crate::prelude::*;
use crate::{process::current_process, syscalls::SyscallHandler};
use kevlar_vfs::stat::{GId, UId};

impl<'a> SyscallHandler<'a> {
    pub fn sys_openat(
        &mut self,
        dirfd: CwdOrFd,
        path: &Path,
        flags: OpenFlags,
        mode: FileMode,
    ) -> Result<isize> {
        let current = current_process();

        // O_TMPFILE: return EOPNOTSUPP so callers fall back to regular temp files.
        // Full O_TMPFILE requires linkat(AT_EMPTY_PATH) which we don't support yet.
        if flags.contains(OpenFlags::O_TMPFILE) {
            return Err(Error::new(Errno::ENOSYS));
        }

        // Reject writes to read-only mounts (MS_RDONLY).
        let access = mode.access_mode();
        if (flags.contains(OpenFlags::O_CREAT) || access == O_WRONLY || access == O_RDWR)
            && path.is_absolute()
            && crate::fs::mount::MountTable::is_readonly(path.as_str())
        {
            return Err(Error::new(Errno::EROFS));
        }

        let is_common_path = path.is_absolute() || matches!(dirfd, CwdOrFd::AtCwd);

        // Single root_fs lock for both O_CREAT and path resolution.
        let path_comp = {
            let root_fs_arc = current.root_fs();
            let root_fs = root_fs_arc.lock_no_irq();

            if flags.contains(OpenFlags::O_CREAT) && !flags.contains(OpenFlags::O_DIRECTORY) {
                // Resolve parent via fast inode path (no chain, no opened_files
                // lock for common absolute/CWD paths).
                let create_result = if is_common_path {
                    root_fs.lookup_parent_inode(path, true)
                } else {
                    let opened_files = current.opened_files_no_irq();
                    root_fs.lookup_parent_inode_at(&opened_files, &dirfd, path, true)
                };

                let effective_mode = FileMode::new(mode.as_u32() & !current.umask());
                match create_result {
                    Ok((parent_inode, name)) => {
                        match parent_inode.as_dir()?.create_file(name, effective_mode, UId::new(current.euid()), GId::new(current.egid())) {
                            Ok(inode) => {
                                // Created — build flat PathComponent, skip re-lookup.
                                root_fs.make_flat_path_component(path, inode)
                            }
                            Err(err)
                                if !flags.contains(OpenFlags::O_EXCL)
                                    && err.errno() == Errno::EEXIST =>
                            {
                                // Exists — look up existing inode, use flat path.
                                let inode = root_fs.lookup_inode(path, true)?;
                                root_fs.make_flat_path_component(path, inode)
                            }
                            Err(err) => return Err(err),
                        }
                    }
                    Err(err) => return Err(err),
                }
            } else if is_common_path {
                // Non-O_CREAT: resolve inode + build flat PathComponent
                // (avoids building intermediate PathComponent chain).
                let inode = root_fs.lookup_inode(path, true)?;
                root_fs.make_flat_path_component(path, inode)
            } else {
                // fd-relative: need opened_files for dirfd resolution.
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
