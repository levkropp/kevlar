// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use super::MAX_READ_WRITE_LEN;
use crate::{fs::opened_file::Fd, prelude::*, user_buffer::UserBufferMut};
use crate::{process::current_process, syscalls::SyscallHandler};
use core::cmp::min;
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    pub fn sys_pread64(
        &mut self,
        fd: Fd,
        uaddr: UserVAddr,
        len: usize,
        offset: usize,
    ) -> Result<isize> {
        let len = min(len, MAX_READ_WRITE_LEN);

        let opened_file = current_process().get_opened_file_by_fd(fd)?;
        let read_len = opened_file
            .as_file()?
            .read(offset, UserBufferMut::from_uaddr(uaddr, len), &opened_file.options())?;

        Ok(read_len as isize)
    }
}
