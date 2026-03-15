// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! waitid(2) — wait for process state change with siginfo_t.
use crate::{
    ctypes::*,
    prelude::*,
    process::{current_process, PId, ProcessState, JOIN_WAIT_QUEUE},
    syscalls::SyscallHandler,
};
use kevlar_platform::address::UserVAddr;

const P_ALL: c_int = 0;
const P_PID: c_int = 1;
const P_PGID: c_int = 2;

const WEXITED: c_int = 4;
const WNOHANG: c_int = 1;
#[allow(dead_code)]
const WNOWAIT: c_int = 0x01000000;

const CLD_EXITED: c_int = 1;
#[allow(dead_code)]
const CLD_KILLED: c_int = 2;
#[allow(dead_code)]
const CLD_STOPPED: c_int = 5;

const SIGCHLD: c_int = 17;

impl<'a> SyscallHandler<'a> {
    pub fn sys_waitid(
        &mut self,
        idtype: c_int,
        id: c_int,
        infop: UserVAddr,
        options: c_int,
    ) -> Result<isize> {
        let want_exited = options & WEXITED != 0;
        let nohang = options & WNOHANG != 0;

        if !want_exited && nohang {
            // WNOHANG with no wait flags: just check, no block.
        }

        let result = JOIN_WAIT_QUEUE.sleep_signalable_until(|| {
            let current = current_process();
            let children = current.children();

            let has_matching = match idtype {
                P_PID => children.iter().any(|c| c.pid().as_i32() == id),
                P_PGID => !children.is_empty(), // simplified
                P_ALL => !children.is_empty(),
                _ => false,
            };
            if !has_matching {
                return Err(Errno::ECHILD.into());
            }

            for child in children.iter() {
                match idtype {
                    P_PID if child.pid().as_i32() != id => continue,
                    _ => {}
                }

                if let ProcessState::ExitedWith(exit_code) = child.state() {
                    if want_exited {
                        return Ok(Some((child.pid(), exit_code, CLD_EXITED, child.uid())));
                    }
                }
            }

            if nohang {
                return Ok(Some((PId::new(0), 0, 0, 0u32)));
            }

            Ok(None)
        })?;

        let (got_pid, status_val, code, uid) = result;

        // Reap the child if it exited.
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

        // Fill siginfo_t by writing individual fields at their offsets.
        if got_pid.as_i32() > 0 {
            // Zero the full 128-byte siginfo first.
            let zeros = [0u8; 128];
            infop.write_bytes(&zeros)?;
            // si_signo at offset 0
            infop.write::<c_int>(&SIGCHLD)?;
            // si_errno at offset 4 (already 0)
            // si_code at offset 8
            infop.add(8).write::<c_int>(&code)?;
            // si_pid at offset 16
            infop.add(16).write::<c_int>(&got_pid.as_i32())?;
            // si_uid at offset 20
            infop.add(20).write::<c_int>(&(uid as c_int))?;
            // si_status at offset 24
            infop.add(24).write::<c_int>(&status_val)?;
        } else {
            // WNOHANG with no child ready: zero out siginfo.
            let zeros = [0u8; 128];
            infop.write_bytes(&zeros)?;
        }

        Ok(0)
    }
}
