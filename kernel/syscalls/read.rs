// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use super::MAX_READ_WRITE_LEN;
use crate::{fs::opened_file::Fd, prelude::*, user_buffer::UserBufferMut};
use crate::{process::current_process, syscalls::SyscallHandler};
use core::cmp::min;
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    pub fn sys_read(&mut self, fd: Fd, uaddr: UserVAddr, len: usize) -> Result<isize> {
        let len = min(len, MAX_READ_WRITE_LEN);

        // Clone the Arc to release the fd table lock before calling read().
        // This avoids deadlock when a procfs read re-locks the fd table
        // (e.g. /proc/self/status reading FDSize).
        let opened_file = current_process().get_opened_file_by_fd(fd)?;
        let read_len = opened_file.read(UserBufferMut::from_uaddr(uaddr, len))?;

        Ok(read_len as isize)
    }
}
