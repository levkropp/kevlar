// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX getpgid(2) man page).
use crate::prelude::*;
use crate::{process::{current_process, Process}, syscalls::SyscallHandler};
use crate::{process::PId, result::Result};

impl<'a> SyscallHandler<'a> {
    pub fn sys_getpgid(&mut self, pid: PId) -> Result<isize> {
        let pgid = if pid.as_i32() == 0 {
            current_process().process_group().lock().pgid()
        } else {
            let proc = Process::find_by_pid(pid)
                .ok_or_else(|| Error::new(Errno::ESRCH))?;
            proc.process_group().lock().pgid()
        };

        Ok(pgid.as_i32() as isize)
    }
}
