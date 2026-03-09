// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::process::{current_process, process_group::{PgId, ProcessGroup}, signal::Signal, PId, Process};
use crate::result::Errno;
use crate::result::Result;
use crate::syscalls::SyscallHandler;

// Provenance: Own (POSIX kill(2) man page).
impl<'a> SyscallHandler<'a> {
    pub fn sys_kill(&self, pid: PId, sig: Signal) -> Result<isize> {
        let pid_int = pid.as_i32();
        match pid_int {
            pid_int if pid_int > 0 => match Process::find_by_pid(pid) {
                Some(proc) => proc.send_signal(sig),
                None => return Err(Errno::ESRCH.into()),
            },
            0 => current_process().process_group().lock().signal(sig),
            -1 => {
                // TODO: check for permissions once linux capabilities is implemented
                current_process().send_signal(sig);
            }
            pid_int if pid_int < -1 => {
                let pgid = PgId::new(-pid_int);
                match ProcessGroup::find_by_pgid(pgid) {
                    Some(pg) => pg.lock().signal(sig),
                    None => return Err(Errno::ESRCH.into()),
                }
            }
            _ => (),
        }

        Ok(0)
    }
}
