// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (Linux fallocate(2) man page).
use crate::fs::opened_file::Fd;
use crate::prelude::*;
use crate::process::current_process;
use crate::syscalls::SyscallHandler;

impl<'a> SyscallHandler<'a> {
    pub fn sys_fallocate(
        &mut self,
        fd: Fd,
        mode: i32,
        offset: i64,
        len: i64,
    ) -> Result<isize> {
        if offset < 0 || len <= 0 {
            return Err(Errno::EINVAL.into());
        }
        // mode=0: allocate space, extend file size if needed.
        // mode=1 (FALLOC_FL_KEEP_SIZE): allocate but don't extend file size.
        // Other modes (PUNCH_HOLE, etc.) not supported yet.
        const FALLOC_FL_KEEP_SIZE: i32 = 0x01;
        if mode != 0 && mode != FALLOC_FL_KEEP_SIZE {
            return Err(Errno::ENOSYS.into());
        }
        let opened_file = current_process().get_opened_file_by_fd(fd)?;
        opened_file.as_file()?.fallocate(offset as usize, len as usize)?;
        Ok(0)
    }
}
