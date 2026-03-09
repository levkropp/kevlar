// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX sigaltstack(2) man page).
use crate::result::Result;
use crate::syscalls::SyscallHandler;

impl<'a> SyscallHandler<'a> {
    pub fn sys_sigaltstack(&mut self, _ss: usize, _old_ss: usize) -> Result<isize> {
        // Stub: accept and ignore. Real alternate signal stack deferred.
        Ok(0)
    }
}
