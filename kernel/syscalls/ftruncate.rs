// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX ftruncate(2) man page).
use crate::fs::opened_file::Fd;
use crate::prelude::*;
use crate::{process::current_process, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_ftruncate(&mut self, fd: Fd, length: usize) -> Result<isize> {
        let opened_file = current_process().get_opened_file_by_fd(fd)?;
        opened_file.as_file()?.truncate(length)?;
        Ok(0)
    }
}
