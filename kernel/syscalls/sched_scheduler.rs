// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::result::Result;
use crate::syscalls::SyscallHandler;

impl<'a> SyscallHandler<'a> {
    /// sched_getscheduler(2) — returns SCHED_OTHER (0).
    pub fn sys_sched_getscheduler(&mut self, _pid: i32) -> Result<isize> {
        Ok(0) // SCHED_OTHER
    }

    /// sched_setscheduler(2) — no-op stub.
    pub fn sys_sched_setscheduler(&mut self, _pid: i32, _policy: i32, _param: usize) -> Result<isize> {
        Ok(0)
    }
}
