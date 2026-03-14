// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::{ctypes::*, process::Process, syscalls::SyscallHandler};

use alloc::string::ToString;

impl<'a> SyscallHandler<'a> {
    pub fn sys_exit(&mut self, status: c_int) -> ! {
        let pid = crate::process::current_process().pid().as_i32();
        let cmd = crate::process::current_process().cmdline().as_str().to_string();
        warn!("exit: pid={} status={} cmd={}", pid, status, cmd);
        Process::exit(status);
    }
}
