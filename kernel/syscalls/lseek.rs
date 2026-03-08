// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Reference: OSv fs/vfs/vfs_syscalls.cc (BSD-3-Clause) — sys_lseek.
// OSv handles SEEK_SET/CUR/END by computing the new offset and clamping
// to >= 0. Adapted for Kevlar's OpenedFile pos tracking.
use crate::ctypes::c_int;
use crate::fs::opened_file::Fd;
use crate::prelude::*;
use crate::{process::current_process, syscalls::SyscallHandler};

const SEEK_SET: c_int = 0;
const SEEK_CUR: c_int = 1;
const SEEK_END: c_int = 2;

impl<'a> SyscallHandler<'a> {
    pub fn sys_lseek(&mut self, fd: Fd, offset: i64, whence: c_int) -> Result<isize> {
        let current = current_process();
        let opened_file = current.get_opened_file_by_fd(fd)?;

        let new_pos: i64 = match whence {
            SEEK_SET => offset,
            SEEK_CUR => opened_file.pos() as i64 + offset,
            SEEK_END => {
                let stat = opened_file.inode().stat()?;
                stat.size.0 as i64 + offset
            }
            _ => return Err(Errno::EINVAL.into()),
        };

        if new_pos < 0 {
            return Err(Errno::EINVAL.into());
        }

        opened_file.set_pos(new_pos as usize);
        Ok(new_pos as isize)
    }
}
