// SPDX-License-Identifier: MIT OR Apache-2.0
use crate::fs::path::Path;
use crate::result::Result;
use crate::{process::current_process, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_chdir(&mut self, path: &Path) -> Result<isize> {
        current_process().root_fs().lock().chdir(path)?;
        Ok(0)
    }
}
