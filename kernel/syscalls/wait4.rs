// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX wait(2)/waitpid(2) man pages, Linux wait status encoding).
use crate::{
    ctypes::*,
    debug,
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
        /// __WCLONE: wait only for children created with clone() that use a
        /// non-default (non-SIGCHLD) exit signal. Since all our children use
        /// SIGCHLD, this flag causes an immediate ECHILD return. musl's
        /// posix_spawn relies on this to detect successful exec.
        const __WCLONE   = 0x8000_0000u32 as i32;
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
        // __WCLONE: only match children with non-SIGCHLD exit signal. All our
        // children use SIGCHLD, so no children match → return ECHILD. musl's
        // posix_spawn uses this to detect that clone+exec succeeded.
        if options.contains(WaitOptions::__WCLONE) {
            return Err(Error::new(Errno::ECHILD));
        }

        let _wait_span = debug::tracer::span_guard(debug::tracer::span::WAIT_TOTAL);
        let (got_pid, encoded_status) = JOIN_WAIT_QUEUE.sleep_signalable_until(|| {
            let current = current_process();
            let children = current.children();

            let has_matching = if pid.as_i32() > 0 {
                children.iter().any(|c| c.pid() == pid)
            } else {
                !children.is_empty()
            };
            if !has_matching {
                return Err(Errno::ECHILD.into());
            }

            for child in children.iter() {
                if pid.as_i32() > 0 && child.pid() != pid {
                    continue;
                }

                match child.state() {
                    ProcessState::ExitedWith(exit_code) => {
                        let ws = (exit_code & 0xff) << 8;
                        return Ok(Some((child.pid(), ws)));
                    }
                    ProcessState::Stopped(stop_sig) => {
                        if options.contains(WaitOptions::WUNTRACED) {
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

        // Evict the child from our children list if it exited (single pass).
        if got_pid.as_i32() > 0 {
            let current = current_process();
            let mut children = current.children();
            if let Some(pos) = children.iter().position(|p| {
                p.pid() == got_pid && matches!(p.state(), ProcessState::ExitedWith(_))
            }) {
                let reaped = children.swap_remove(pos);
                // Move reaped process to EXITED_PROCESSES for deferred kernel
                // stack cleanup (gc'd from idle thread).
                crate::process::EXITED_PROCESSES.lock().push(reaped);
            }
        }

        if let Some(status) = status {
            debug::usercopy::set_context("sys_wait4:status");
            let r = status.write::<c_int>(&encoded_status);
            debug::usercopy::clear_context();
            r?;
        }
        Ok(got_pid.as_i32() as isize)
    }
}
