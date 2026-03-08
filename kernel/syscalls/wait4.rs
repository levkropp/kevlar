// SPDX-License-Identifier: MIT OR Apache-2.0
use crate::{
    ctypes::*,
    prelude::*,
    process::{current_process, PId, ProcessState, JOIN_WAIT_QUEUE},
    syscalls::SyscallHandler,
};

use bitflags::bitflags;
use kevlar_runtime::address::UserVAddr;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct WaitOptions: c_int {
        const WNOHANG   = 1;
        const WUNTRACED = 2;
    }
}

impl<'a> SyscallHandler<'a> {
    pub fn sys_wait4(
        &mut self,
        pid: PId,
        status: Option<UserVAddr>,
        options: WaitOptions,
        _rusage: Option<UserVAddr>,
    ) -> Result<isize> {
        let (got_pid, status_value) = JOIN_WAIT_QUEUE.sleep_signalable_until(|| {
            let current = current_process();
            for child in current.children().iter() {
                if pid.as_i32() > 0 && child.pid() != pid {
                    // Wait for the specific PID.
                    continue;
                }

                // pid == -1: wait for any child (most common case from musl).
                // pid == 0: wait for children in same process group (treat as any).
                // pid > 0: wait for specific child (handled above).

                if let ProcessState::ExitedWith(status_value) = child.state() {
                    return Ok(Some((child.pid(), status_value)));
                }
            }

            if options.contains(WaitOptions::WNOHANG) {
                return Ok(Some((PId::new(0), 0)));
            }

            Ok(None)
        })?;

        // Evict the joined process object.
        current_process().children().retain(|p| p.pid() != got_pid);

        if let Some(status) = status {
            // Linux wait status encoding: normal exit is (exit_code << 8) | 0.
            // Signal death would be raw signal number in low 7 bits.
            let encoded_status = (status_value & 0xff) << 8;
            status.write::<c_int>(&encoded_status)?;
        }
        Ok(got_pid.as_i32() as isize)
    }
}
