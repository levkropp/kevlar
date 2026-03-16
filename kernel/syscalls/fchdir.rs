// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX fchdir(2) man page).
use crate::fs::opened_file::Fd;
use crate::prelude::*;
use crate::{process::current_process, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_fchdir(&mut self, fd: Fd) -> Result<isize> {
        let current = current_process();
        let path = {
            let opened_files = current.opened_files().lock();
            let opened_file = opened_files.get(fd)?;
            opened_file.path().resolve_absolute_path()
        };
        let root_fs = current.root_fs();
        root_fs.lock().chdir(&path)?;
        Ok(0)
    }
}
