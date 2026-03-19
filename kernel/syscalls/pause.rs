// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX pause(2) man page).
// Suspends the process until a signal is delivered.
use crate::{prelude::*, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_pause(&mut self) -> Result<isize> {
        // Block until a signal arrives. sleep_signalable_until returns
        // Err(EINTR) when has_pending_signals() becomes true.
        crate::poll::POLL_WAIT_QUEUE.sleep_signalable_until(|| Ok(None))?;
        // Unreachable: sleep_signalable_until always returns Err(EINTR) here.
        Err(Errno::EINTR.into())
    }
}
