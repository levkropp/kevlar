// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::result::Result;
use crate::{process::current_process, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_getpid(&mut self) -> Result<isize> {
        // ns_pid == tgid in root namespace, namespace-local PID otherwise.
        // Avoids cloning the full NamespaceSet (3 Arc increments).
        Ok(current_process().ns_pid().as_i32() as isize)
    }
}
