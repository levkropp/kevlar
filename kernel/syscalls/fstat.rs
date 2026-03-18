// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::fs::opened_file::Fd;
use crate::result::Result;
use crate::{process::current_process, syscalls::SyscallHandler};
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    pub fn sys_fstat(&mut self, fd: Fd, buf: UserVAddr) -> Result<isize> {
        current_process().with_file(fd, |opened_file| {
            let stat = opened_file.path().inode.stat()?;
            buf.write(&stat)?;
            Ok(0)
        })
    }
}
