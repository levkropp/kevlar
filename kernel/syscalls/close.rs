// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::{fs::opened_file::Fd, result::Result};
use crate::{process::current_process, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_close(&mut self, fd: Fd) -> Result<isize> {
        let proc = current_process();
        let pid = proc.pid().as_i32();
        // Trace Xorg (PID 3) closing client fds to understand why xterm disconnects
        if pid == 3 && fd.as_int() >= 7 {
            warn!("close(fd={}) from Xorg — xterm disconnect?", fd.as_int());
        }
        #[cfg(not(feature = "profile-fortress"))]
        proc.invalidate_hot_fd(fd.as_int());
        proc.opened_files_no_irq().close(fd)?;
        Ok(0)
    }
}
