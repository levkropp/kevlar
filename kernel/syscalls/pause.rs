// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX pause(2) man page).
// Suspends the process until a signal is delivered.
use crate::{prelude::*, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_pause(&mut self) -> Result<isize> {
        // Yield to the scheduler; signal delivery happens on syscall return.
        crate::process::switch();
        Err(Errno::EINTR.into())
    }
}
