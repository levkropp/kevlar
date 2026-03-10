// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::debug;
use crate::fs::opened_file::Fd;
use crate::result::Result;
use crate::{process::current_process, syscalls::SyscallHandler};
use core::mem::size_of;
use kevlar_platform::address::UserVAddr;
use kevlar_utils::alignment::align_up;

use crate::user_buffer::UserBufWriter;

impl<'a> SyscallHandler<'a> {
    pub fn sys_getdents64(&mut self, fd: Fd, dirp: UserVAddr, len: usize) -> Result<isize> {
        // Clone the Arc to release the fd table lock before iterating.
        // This avoids deadlock when a procfs directory re-locks the fd table
        // (e.g. /proc/[pid]/fd/ enumerating open file descriptors).
        let dir = current_process().get_opened_file_by_fd(fd)?;
        debug::usercopy::set_context("sys_getdents64");
        let mut writer = UserBufWriter::from_uaddr(dirp, len);
        while let Some(entry) = dir.readdir()? {
            let alignment = size_of::<u64>();
            let reclen = align_up(
                size_of::<u64>() * 2 + size_of::<u16>() + 1 + entry.name.len() + 1,
                alignment,
            );

            if writer.pos() + reclen > len {
                break;
            }

            // Fill a `struct linux_dirent64`.
            // d_ino
            writer.write::<u64>(entry.inode_no.as_u64())?;
            // d_off
            writer.write::<u64>(dir.pos() as u64)?;
            // d_reclen
            writer.write::<u16>(reclen as u16)?;
            // d_type
            writer.write::<u8>(entry.file_type as u8)?;
            // d_name
            writer.write_bytes(entry.name.as_bytes())?;
            // d_name (null character)
            writer.write::<u8>(0)?;

            writer.skip_until_alignment(alignment)?;
        }

        debug::usercopy::clear_context();
        Ok(writer.pos() as isize)
    }
}
