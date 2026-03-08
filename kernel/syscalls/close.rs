// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::{fs::opened_file::Fd, result::Result};
use crate::{process::current_process, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_close(&mut self, fd: Fd) -> Result<isize> {
        current_process().opened_files().lock().close(fd)?;
        Ok(0)
    }
}
