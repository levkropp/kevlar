// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::fs::opened_file::{Fd, OpenOptions};
use crate::prelude::*;
use crate::process::current_process;
use crate::syscalls::SyscallHandler;

impl<'a> SyscallHandler<'a> {
    pub fn sys_dup(&mut self, fd: Fd) -> Result<isize> {
        let current = current_process();
        let mut opened_files = current.opened_files_no_irq();
        let new_fd = opened_files.dup(fd, None, OpenOptions::empty())?;
        Ok(new_fd.as_int() as isize)
    }
}
