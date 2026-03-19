// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::fs::opened_file::{Fd, OpenOptions};
use crate::prelude::*;
use crate::process::current_process;
use crate::syscalls::SyscallHandler;

impl<'a> SyscallHandler<'a> {
    pub fn sys_dup2(&mut self, old: Fd, new: Fd) -> Result<isize> {
        let current = current_process();
        // dup2 replaces `new` fd — invalidate if it was cached.
        #[cfg(not(feature = "profile-fortress"))]
        current.invalidate_hot_fd(new.as_int());
        let mut opened_files = current.opened_files().lock();
        opened_files.dup2(old, new, OpenOptions::new(false, false))?;
        Ok(new.as_int() as isize)
    }
}
