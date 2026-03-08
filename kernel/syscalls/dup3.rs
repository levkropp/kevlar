// SPDX-License-Identifier: MIT OR Apache-2.0
use crate::fs::opened_file::{Fd, OpenFlags, OpenOptions};
use crate::prelude::*;
use crate::process::current_process;
use crate::result::Errno;
use crate::syscalls::SyscallHandler;

impl<'a> SyscallHandler<'a> {
    pub fn sys_dup3(&mut self, old: Fd, new: Fd, flags: i32) -> Result<isize> {
        if old.as_int() == new.as_int() {
            return Err(Errno::EINVAL.into());
        }

        let cloexec = (flags & OpenFlags::O_CLOEXEC.bits()) != 0;
        let options = OpenOptions::new(false, cloexec);

        let current = current_process();
        let mut opened_files = current.opened_files().lock();
        opened_files.dup2(old, new, options)?;
        Ok(new.as_int() as isize)
    }
}
