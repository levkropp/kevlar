// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::fs::path::Path;
use crate::result::Result;
use crate::{process::current_process, syscalls::SyscallHandler};
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    pub fn sys_lstat(&mut self, path: &Path, buf: UserVAddr) -> Result<isize> {
        let root_fs = current_process().root_fs();
        let stat = root_fs
            .lock_no_irq()
            .lookup_no_symlink_follow(path)?
            .stat()?;
        buf.write(&stat.to_abi_bytes())?;
        Ok(0)
    }
}
