// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::{process::current_process, result::Result, syscalls::SyscallHandler};

/// PRIO_PROCESS: target is a process (PID-based). We only support self (who=0).
const PRIO_PROCESS: i32 = 0;

impl<'a> SyscallHandler<'a> {
    /// sys_getpriority: returns 20 - nice (per Linux kernel convention).
    /// The libc wrapper inverts this back to the nice value, so the user sees 0..19.
    pub fn sys_getpriority(&mut self, _which: i32, _who: i32) -> Result<isize> {
        let nice = current_process().nice();
        Ok((20 - nice) as isize)
    }

    /// sys_setpriority: prio is the nice value (-20 to +19).
    pub fn sys_setpriority(&mut self, _which: i32, _who: i32, prio: i32) -> Result<isize> {
        let _ = PRIO_PROCESS; // suppress unused warning
        current_process().set_nice(prio.clamp(-20, 19));
        Ok(0)
    }
}
