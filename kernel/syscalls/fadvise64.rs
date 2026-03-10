// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (Linux fadvise64(2) man page).
use crate::fs::opened_file::Fd;
use crate::prelude::*;
use crate::process::current_process;
use crate::syscalls::SyscallHandler;

impl<'a> SyscallHandler<'a> {
    pub fn sys_fadvise64(
        &mut self,
        fd: Fd,
        _offset: i64,
        _len: i64,
        _advice: i32,
    ) -> Result<isize> {
        // Validate the fd exists.
        let _opened_file = current_process().get_opened_file_by_fd(fd)?;
        // Stub: advisory only, no effect needed.
        Ok(0)
    }
}
