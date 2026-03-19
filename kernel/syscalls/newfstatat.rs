// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
// Own implementation based on Linux man pages.
use super::CwdOrFd;
use crate::ctypes::c_int;
use crate::fs::path::Path;
use crate::prelude::*;
use crate::{process::current_process, syscalls::SyscallHandler};
use kevlar_platform::address::UserVAddr;

const AT_EMPTY_PATH: c_int = 0x1000;
const AT_SYMLINK_NOFOLLOW: c_int = 0x100;

impl<'a> SyscallHandler<'a> {
    pub fn sys_newfstatat(
        &mut self,
        dirfd: CwdOrFd,
        path: &Path,
        buf: UserVAddr,
        flags: c_int,
    ) -> Result<isize> {
        let current = current_process();
        let follow_symlink = (flags & AT_SYMLINK_NOFOLLOW) == 0;

        let stat = if (flags & AT_EMPTY_PATH) != 0 && path.as_str().is_empty() {
            // AT_EMPTY_PATH: stat the fd itself.
            match &dirfd {
                CwdOrFd::Fd(fd) => current.get_opened_file_by_fd(*fd)?.inode().stat()?,
                CwdOrFd::AtCwd => {
                    let root_fs_arc = current.root_fs();
                    let root_fs = root_fs_arc.lock_no_irq();
                    root_fs.lookup(Path::new("/"))?.stat()?
                }
            }
        } else {
            let root_fs_arc = current.root_fs();
            let root_fs = root_fs_arc.lock_no_irq();
            let opened_files = current.opened_files_no_irq();
            let path_comp = root_fs.lookup_path_at(&opened_files, &dirfd, path, follow_symlink)?;
            path_comp.inode.stat()?
        };

        buf.write(&stat.to_abi_bytes())?;
        Ok(0)
    }
}
