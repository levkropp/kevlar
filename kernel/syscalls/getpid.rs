// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::result::Result;
use crate::{process::current_process, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_getpid(&mut self) -> Result<isize> {
        // For threads, getpid() returns the thread group ID (TGID).
        // In a PID namespace, return the namespace-local PID.
        let proc = current_process();
        let ns = proc.namespaces();
        if ns.pid_ns.is_root() {
            Ok(proc.tgid().as_i32() as isize)
        } else {
            Ok(proc.ns_pid().as_i32() as isize)
        }
    }
}
