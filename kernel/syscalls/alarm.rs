// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX alarm(2) man page).
// Stub — returns 0 (no previous alarm was pending).
use crate::{prelude::*, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_alarm(&mut self, _seconds: u32) -> Result<isize> {
        // TODO: Implement real alarm delivery via SIGALRM.
        Ok(0)
    }
}
