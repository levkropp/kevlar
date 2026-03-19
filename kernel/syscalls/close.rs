// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::{fs::opened_file::Fd, result::Result};
use crate::{process::current_process, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_close(&mut self, fd: Fd) -> Result<isize> {
        let proc = current_process();
        #[cfg(not(feature = "profile-fortress"))]
        proc.invalidate_hot_fd(fd.as_int());
        proc.opened_files_no_irq().close(fd)?;
        Ok(0)
    }
}
