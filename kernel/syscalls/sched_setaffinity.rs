// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::result::Result;
use crate::syscalls::SyscallHandler;

impl<'a> SyscallHandler<'a> {
    /// sched_setaffinity(2) — set CPU affinity mask. No-op stub.
    pub fn sys_sched_setaffinity(&mut self, _pid: i32, _cpusetsize: usize, _mask: usize) -> Result<isize> {
        Ok(0)
    }
}
