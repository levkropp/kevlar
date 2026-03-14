// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! pidfd_open(2) — process file descriptor (stub).
//! Returns ENOSYS for now. systemd handles this gracefully and falls
//! back to SIGCHLD-based process monitoring.
use crate::{result::{Errno, Result}, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_pidfd_open(&mut self, _pid: i32, _flags: u32) -> Result<isize> {
        Err(Errno::ENOSYS.into())
    }
}
