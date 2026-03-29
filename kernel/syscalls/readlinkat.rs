// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX readlinkat(2) man page).
use super::CwdOrFd;
use crate::fs::path::Path;
use crate::prelude::*;
use crate::user_buffer::UserBufWriter;
use crate::{process::current_process, syscalls::SyscallHandler};
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    pub fn sys_readlinkat(
        &mut self,
        dirfd: CwdOrFd,
        path: &Path,
        buf: UserVAddr,
        buf_size: usize,
    ) -> Result<isize> {
        // Handle /proc/self/fd/N
        if path.as_str().starts_with("/proc/self/fd/") {
            return self.sys_readlink(path, buf, buf_size);
        }

        let current = current_process();
        let root_fs_arc = current.root_fs();
        let root_fs = root_fs_arc.lock();
        // Fast path: use lookup_inode to avoid PathComponent heap allocations.
        // Falls back to full lookup_path for ".." or intermediate symlinks.
        let inode = if path.is_absolute() || matches!(dirfd, CwdOrFd::AtCwd) {
            root_fs.lookup_inode(path, false)?
        } else {
            let opened_files = current.opened_files_no_irq();
            root_fs.lookup_path_at(&opened_files, &dirfd, path, false)?.inode.clone()
        };
        let resolved = inode.readlink()?;
        let bytes = resolved.as_bytes();

        // POSIX: readlink truncates if buf_size is too small (no error).
        // The caller detects truncation by checking if retval == buf_size.
        let copy_len = core::cmp::min(bytes.len(), buf_size);
        let mut writer = UserBufWriter::from_uaddr(buf, buf_size);
        writer.write_bytes(&bytes[..copy_len])?;
        // POSIX: readlink does NOT write a NUL terminator.
        Ok(copy_len as isize)
    }
}
