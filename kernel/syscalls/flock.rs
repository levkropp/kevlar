// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! flock(2) — advisory file locking (stub).
use crate::{fs::opened_file::Fd, process::current_process, result::Result, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_flock(&mut self, fd: i32, _operation: i32) -> Result<isize> {
        // Validate the fd exists (returns EBADF if closed).
        let _ = current_process().get_opened_file_by_fd(Fd::new(fd))?;
        // Advisory locking is a no-op — accept silently.
        Ok(0)
    }
}
