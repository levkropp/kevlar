// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use super::MAX_READ_WRITE_LEN;
use crate::{fs::opened_file::Fd, prelude::*, user_buffer::UserBufferMut};
use crate::{process::current_process, syscalls::SyscallHandler};
use core::cmp::min;
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    pub fn sys_read(&mut self, fd: Fd, uaddr: UserVAddr, len: usize) -> Result<isize> {
        let len = min(len, MAX_READ_WRITE_LEN);

        // Hold the fd table guard across the read to avoid Arc::clone/drop
        // overhead (~20ns).  Safe on single-CPU: nothing can modify the fd
        // table while this syscall is executing.
        let opened_files = current_process().opened_files_no_irq();
        let opened_file = opened_files.get(fd)?;
        let read_len = opened_file.read(UserBufferMut::from_uaddr(uaddr, len))?;

        // MAX_READ_WRITE_LEN limit guarantees total_len is in the range of isize.
        Ok(read_len as isize)
    }
}
