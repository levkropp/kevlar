// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::result::Result;
use crate::{process::current_process, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_getpid(&mut self) -> Result<isize> {
        let current = current_process();
        let tgid = current.tgid();
        // Fast path: non-thread in root namespace (the common case).
        // ns_pid equals the global PID in root namespace, and for the
        // group leader pid == tgid, so ns_pid is already correct.
        if current.pid() == tgid {
            return Ok(current.ns_pid().as_i32() as isize);
        }
        // Thread: getpid() must return the tgid, not the thread's own PID.
        // In non-root PID namespaces, translate tgid to the local PID.
        let ns = current.namespaces();
        if ns.pid_ns.is_root() {
            Ok(tgid.as_i32() as isize)
        } else {
            let ns_tgid = ns.pid_ns.global_to_local(tgid)
                .unwrap_or(tgid);
            Ok(ns_tgid.as_i32() as isize)
        }
    }
}
