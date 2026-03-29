// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX setsid(2) man page).
use crate::prelude::*;
use crate::process::process_group::{PgId, ProcessGroup};
use crate::process::current_process;
use crate::result::Result;
use crate::syscalls::SyscallHandler;

impl<'a> SyscallHandler<'a> {
    pub fn sys_setsid(&mut self) -> Result<isize> {
        let proc = current_process();
        let pid = proc.pid().as_i32();

        // setsid fails if the caller is already a process group leader.
        let current_pgid = proc.process_group().lock().pgid().as_i32();
        if current_pgid == pid {
            return Err(Errno::EPERM.into());
        }

        // Create a new session and process group with pgid == pid.
        let proc_weak = Arc::downgrade(&proc);
        let old_pg = proc.process_group();
        let new_pg = ProcessGroup::find_or_create_by_pgid(PgId::new(pid));

        old_pg.lock().remove(&proc_weak);
        new_pg.lock().add(proc_weak);
        proc.set_process_group(Arc::downgrade(&new_pg));

        // Set the session ID to caller's PID (caller becomes session leader).
        proc.set_session_id(pid);

        Ok(pid as isize)
    }
}
