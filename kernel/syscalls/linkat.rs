// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::fs::opened_file::Fd;
use crate::fs::path::Path;
use crate::result::{Errno, Result};
use crate::{
    process::current_process,
    syscalls::{AtFlags, CwdOrFd, SyscallHandler},
};

const AT_EMPTY_PATH: i32 = 0x1000;

impl<'a> SyscallHandler<'a> {
    pub fn sys_linkat(
        &mut self,
        src_dir: CwdOrFd,
        src_path: &Path,
        dst_dir: CwdOrFd,
        dst_path: &Path,
        flags: AtFlags,
    ) -> Result<isize> {
        let current = current_process();
        let root_fs_arc = current.root_fs();
        let root_fs = root_fs_arc.lock();
        let opened_files = current.opened_files().lock();

        // AT_EMPTY_PATH: use the fd itself as the source inode.
        let raw_flags = flags.bits();
        let src = if (raw_flags & AT_EMPTY_PATH) != 0 && src_path.as_str().is_empty() {
            match &src_dir {
                CwdOrFd::Fd(fd) => {
                    let opened_file = opened_files.get(*fd)?;
                    opened_file.path().clone()
                }
                _ => return Err(Errno::EINVAL.into()),
            }
        } else {
            root_fs.lookup_path_at(
                &opened_files,
                &src_dir,
                src_path,
                flags.contains(AtFlags::AT_SYMLINK_FOLLOW),
            )?
        };

        let (parent_inode, dst_name) =
            root_fs.lookup_parent_inode_at(&opened_files, &dst_dir, dst_path, true)?;
        parent_inode.as_dir()?.link(dst_name, &src.inode)?;
        Ok(0)
    }
}
