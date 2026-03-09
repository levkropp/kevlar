// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX sigsuspend(2) man page).
// Temporarily replaces the signal mask and suspends until a signal is delivered.
use crate::{
    prelude::*,
    process::{current_process, signal::SigSet},
    syscalls::SyscallHandler,
};
use kevlar_runtime::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    pub fn sys_rt_sigsuspend(&mut self, mask_ptr: UserVAddr, _sigsetsize: usize) -> Result<isize> {
        let new_mask = mask_ptr.read::<[u8; 128]>()?;
        let new_mask = SigSet::new(new_mask);

        let current = current_process();

        // Save old mask and set new one.
        let old_mask = {
            let mut sigset = current.sigset_lock();
            let old = *sigset;
            *sigset = new_mask;
            old
        };

        // Sleep until a signal arrives. We use a simple yield loop:
        // the signal will be delivered in try_delivering_signal on return
        // from the syscall.
        crate::process::switch();

        // Restore old mask.
        {
            let mut sigset = current.sigset_lock();
            *sigset = old_mask;
        }

        // sigsuspend always returns EINTR.
        Err(Errno::EINTR.into())
    }
}
