// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::fs::stat::{O_RDWR, O_WRONLY};
use crate::fs::{inotify, opened_file::OpenFlags, path::Path, stat::FileMode};
use crate::prelude::*;
use crate::timer::read_wall_clock;
use crate::{process::current_process, syscalls::SyscallHandler};
use kevlar_vfs::stat::{GId, UId};

impl<'a> SyscallHandler<'a> {
    pub fn sys_open(&mut self, path: &Path, flags: OpenFlags, mode: FileMode) -> Result<isize> {
        let current = current_process();

        // Reject writes to read-only mounts (MS_RDONLY).
        let access = mode.access_mode();
        if (flags.contains(OpenFlags::O_CREAT) || access == O_WRONLY || access == O_RDWR)
            && crate::fs::mount::MountTable::is_readonly(path.as_str())
        {
            return Err(Error::new(Errno::EROFS));
        }

        let path_comp = {
            let root_fs_arc = current.root_fs();
            let root_fs = root_fs_arc.lock_no_irq();

            if flags.contains(OpenFlags::O_CREAT) && !flags.contains(OpenFlags::O_DIRECTORY) {
                let (parent_inode, name) = root_fs.lookup_parent_inode(path, true)?;
                let effective_mode = FileMode::new(mode.as_u32() & !current.umask());
                match parent_inode.as_dir()?.create_file(name, effective_mode, UId::new(current.euid()), GId::new(current.egid())) {
                    Ok(inode) => {
                        let now = read_wall_clock().secs_from_epoch() as isize;
                        let _ = inode.set_times(Some(now), Some(now));
                        if let Some((parent, fname)) = path.parent_and_basename() {
                            inotify::notify(parent.as_str(), fname, inotify::IN_CREATE);
                        }
                        root_fs.make_flat_path_component(path, inode)
                    }
                    Err(err)
                        if !flags.contains(OpenFlags::O_EXCL)
                            && err.errno() == Errno::EEXIST =>
                    {
                        let inode = root_fs.lookup_inode(path, true)?;
                        root_fs.make_flat_path_component(path, inode)
                    }
                    Err(err) => return Err(err),
                }
            } else {
                // Non-O_CREAT: resolve inode + build flat PathComponent.
                let inode = root_fs.lookup_inode(path, true)?;
                root_fs.make_flat_path_component(path, inode)
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
