// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX getsid(2) man page).
use crate::prelude::*;
use crate::process::{current_process, PId, Process};
use crate::result::Result;
use crate::syscalls::SyscallHandler;

impl<'a> SyscallHandler<'a> {
    pub fn sys_getsid(&mut self, pid: PId) -> Result<isize> {
        // Simplified: return the process group leader's PID as the session ID.
        // A proper implementation would track sessions separately.
        let pgid = if pid.as_i32() == 0 {
            current_process().process_group().lock().pgid().as_i32()
        } else {
            let proc = Process::find_by_pid(pid)
                .ok_or_else(|| Error::new(Errno::ESRCH))?;
            proc.process_group().lock().pgid().as_i32()
        };
        Ok(pgid as isize)
    }
}
