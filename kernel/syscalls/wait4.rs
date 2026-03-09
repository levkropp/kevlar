// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX wait(2)/waitpid(2) man pages, Linux wait status encoding).
use crate::{
    ctypes::*,
    prelude::*,
    process::{current_process, PId, ProcessState, JOIN_WAIT_QUEUE},
    syscalls::SyscallHandler,
};

use bitflags::bitflags;
use kevlar_platform::address::UserVAddr;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct WaitOptions: c_int {
        const WNOHANG    = 1;
        const WUNTRACED  = 2;
        const WCONTINUED = 8;
    }
}

/// Wait status encoding (matches Linux):
///   Normal exit:  (exit_code << 8) | 0x00       — WIFEXITED, WEXITSTATUS
///   Signal death: 0x00 | (signo & 0x7f)         — WIFSIGNALED, WTERMSIG
///   Stopped:      (signo << 8) | 0x7f           — WIFSTOPPED, WSTOPSIG
///   Continued:    0xffff                         — WIFCONTINUED

impl<'a> SyscallHandler<'a> {
    pub fn sys_wait4(
        &mut self,
        pid: PId,
        status: Option<UserVAddr>,
        options: WaitOptions,
        _rusage: Option<UserVAddr>,
    ) -> Result<isize> {
        let (got_pid, encoded_status) = JOIN_WAIT_QUEUE.sleep_signalable_until(|| {
            let current = current_process();
            for child in current.children().iter() {
                if pid.as_i32() > 0 && child.pid() != pid {
                    continue;
                }

                // pid == -1: wait for any child.
                // pid == 0: wait for children in same process group (treat as any for now).
                // pid > 0: wait for specific child (handled above).

                match child.state() {
                    ProcessState::ExitedWith(exit_code) => {
                        // Normal exit: (exit_code << 8) | 0x00
                        let ws = (exit_code & 0xff) << 8;
                        return Ok(Some((child.pid(), ws)));
                    }
                    ProcessState::Stopped(stop_sig) => {
                        if options.contains(WaitOptions::WUNTRACED) {
                            // Stopped: (signo << 8) | 0x7f
                            let ws = ((stop_sig & 0xff) << 8) | 0x7f;
                            return Ok(Some((child.pid(), ws)));
                        }
                    }
                    _ => {}
                }
            }

            if options.contains(WaitOptions::WNOHANG) {
                return Ok(Some((PId::new(0), 0)));
            }

            Ok(None)
        })?;

        // Only evict the child if it actually exited (not just stopped).
        if got_pid.as_i32() > 0 {
            let current = current_process();
            let should_evict = current
                .children()
                .iter()
                .any(|p| p.pid() == got_pid && matches!(p.state(), ProcessState::ExitedWith(_)));
            if should_evict {
                current.children().retain(|p| p.pid() != got_pid);
            }
        }

        if let Some(status) = status {
            status.write::<c_int>(&encoded_status)?;
        }
        Ok(got_pid.as_i32() as isize)
    }
}
