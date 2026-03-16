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
        let opened_files = current.opened_files().lock();
        let path_comp = root_fs.lookup_path_at(&opened_files, &dirfd, path, false)?;
        let resolved = path_comp.inode.readlink()?;

        if buf_size < resolved.as_str().as_bytes().len() {
            return Err(Errno::ERANGE.into());
        }

        let mut writer = UserBufWriter::from_uaddr(buf, buf_size);
        writer.write_bytes(resolved.as_str().as_bytes())?;
        writer.write(0u8)?;
        Ok(writer.pos() as isize)
    }
}
