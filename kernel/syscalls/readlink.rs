// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use kevlar_platform::address::UserVAddr;

use crate::result::Result;
use crate::syscalls::SyscallHandler;
use crate::{
    fs::{opened_file::Fd, path::Path},
    process::current_process,
    result::Errno,
};

use crate::user_buffer::UserBufWriter;

impl<'a> SyscallHandler<'a> {
    pub fn sys_readlink(&mut self, path: &Path, buf: UserVAddr, buf_size: usize) -> Result<isize> {
        // Handle /proc/self/fd/N: resolve from the fd table directly.
        if path.as_str().starts_with("/proc/self/fd/") {
            let fd = path.as_str()["/proc/self/fd/".len()..].parse().unwrap();
            let resolved_path = current_process()
                .opened_files_no_irq()
                .get(Fd::new(fd))?
                .path()
                .resolve_absolute_path();
            let bytes = resolved_path.as_str().as_bytes();
            // POSIX: readlink truncates if buf_size is too small (no error).
            let copy_len = core::cmp::min(bytes.len(), buf_size);
            let mut writer = UserBufWriter::from_uaddr(buf, buf_size);
            writer.write_bytes(&bytes[..copy_len])?;
            // POSIX: readlink does NOT write a NUL terminator.
            return Ok(copy_len as isize);
        }

        let root_fs = current_process().root_fs();
        let inode = root_fs
            .lock_no_irq()
            .lookup_no_symlink_follow(path)?;
        let resolved = inode.readlink()?;
        let bytes = resolved.as_bytes();
        // POSIX: readlink truncates if buf_size is too small (no error).
        let copy_len = core::cmp::min(bytes.len(), buf_size);
        let mut writer = UserBufWriter::from_uaddr(buf, buf_size);
        writer.write_bytes(&bytes[..copy_len])?;
        // POSIX: readlink does NOT write a NUL terminator.
        Ok(copy_len as isize)
    }
}
