// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX tgkill(2) man page).
// Since we don't have real threads yet, tgkill behaves like kill(tid, sig).
use crate::{
    ctypes::*,
    prelude::*,
    process::{PId, Process},
    syscalls::SyscallHandler,
};

impl<'a> SyscallHandler<'a> {
    pub fn sys_tkill(&mut self, tid: c_int, sig: c_int) -> Result<isize> {
        self.sys_tgkill(0, tid, sig)
    }

    pub fn sys_tgkill(&mut self, _tgid: c_int, tid: c_int, sig: c_int) -> Result<isize> {
        if sig == 0 {
            // Signal 0 is used to check if the process exists.
            return match Process::find_by_pid(PId::new(tid)) {
                Some(_) => Ok(0),
                None => Err(Errno::ESRCH.into()),
            };
        }

        match Process::find_by_pid(PId::new(tid)) {
            Some(proc) => {
                proc.send_signal(sig);
                Ok(0)
            }
            None => Err(Errno::ESRCH.into()),
        }
    }
}
