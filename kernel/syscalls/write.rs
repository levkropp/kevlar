// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use super::MAX_READ_WRITE_LEN;
use crate::prelude::*;
use crate::{fs::opened_file::Fd, user_buffer::UserBuffer};
use crate::{process::current_process, syscalls::SyscallHandler};
use core::cmp::min;
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    pub fn sys_write(&mut self, fd: Fd, uaddr: UserVAddr, len: usize) -> Result<isize> {
        let len = min(len, MAX_READ_WRITE_LEN);

        // Hold the fd table guard across the write to avoid Arc::clone/drop.
        let opened_files = current_process().opened_files_no_irq();
        let opened_file = opened_files.get(fd)?;
        let written_len = opened_file.write(UserBuffer::from_uaddr(uaddr, len))?;

        // MAX_READ_WRITE_LEN limit guarantees total_len is in the range of isize.
        Ok(written_len as isize)
    }
}
