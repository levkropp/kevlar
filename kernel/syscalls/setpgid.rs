// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::{
    prelude::*,
    process::process_group::PgId,
    process::{current_process, process_group::ProcessGroup, PId, Process},
    result::Result,
    syscalls::SyscallHandler,
};

impl<'a> SyscallHandler<'a> {
    pub fn sys_setpgid(&mut self, pid: PId, pgid: PgId) -> Result<isize> {
        let target = if pid.as_i32() == 0 {
            current_process().clone()
        } else {
            Process::find_by_pid(pid).ok_or_else(|| Error::new(Errno::ESRCH))?
        };

        // pgid=0 means "use the target process's PID as the new pgid" (POSIX).
        let effective_pgid = if pgid.as_i32() == 0 {
            PgId::new(target.pid().as_i32())
        } else {
            pgid
        };

        let new_pg = ProcessGroup::find_or_create_by_pgid(effective_pgid);
        let proc_weak = Arc::downgrade(&target);
        let old_pg = target.process_group();

        if !Arc::ptr_eq(&old_pg, &new_pg) {
            old_pg.lock().remove(&proc_weak);
            new_pg.lock().add(proc_weak);
            target.set_process_group(Arc::downgrade(&new_pg));
        }

        Ok(effective_pgid.as_i32() as isize)
    }
}
