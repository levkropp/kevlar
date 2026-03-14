// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! close_range(2) — close a range of file descriptors.
use crate::{
    fs::opened_file::Fd,
    process::current_process,
    result::Result,
    syscalls::SyscallHandler,
};

impl<'a> SyscallHandler<'a> {
    pub fn sys_close_range(&mut self, first: u32, last: u32, _flags: u32) -> Result<isize> {
        let proc = current_process();
        let mut files = proc.opened_files().lock();
        let max_fd = files.table_size() as u32;
        let end = core::cmp::min(last, max_fd.saturating_sub(1));

        for fd_num in first..=end {
            let _ = files.close(Fd::new(fd_num as i32));
        }

        Ok(0)
    }
}
