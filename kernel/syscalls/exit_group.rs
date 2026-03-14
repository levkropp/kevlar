// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::{ctypes::*, process::Process, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_exit_group(&mut self, status: c_int) -> ! {
        let pid = crate::process::current_process().pid().as_i32();
        let cmd = crate::process::current_process().cmdline().as_str().to_string();
        warn!("exit_group: pid={} status={} cmd={}", pid, status, cmd);
        Process::exit_group(status);
    }
}

use alloc::string::ToString;
