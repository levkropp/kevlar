// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX sigsuspend(2) man page).
// Temporarily replaces the signal mask and suspends until a signal is delivered.
use crate::{
    prelude::*,
    process::{current_process, signal::SigSet},
    syscalls::SyscallHandler,
};
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    pub fn sys_rt_sigsuspend(&mut self, mask_ptr: UserVAddr, _sigsetsize: usize) -> Result<isize> {
        let new_bytes = mask_ptr.read::<[u8; 8]>()?;
        let new_mask = SigSet::from_bytes(&new_bytes);

        let current = current_process();

        // Save old mask and set new one (lock-free).
        let old_mask = current.sigset_load();
        current.sigset_store(new_mask);

        // Sleep until a signal arrives. The signal will be delivered in
        // try_delivering_signal on return from the syscall.
        crate::process::switch();

        // Restore old mask.
        current.sigset_store(old_mask);

        // sigsuspend always returns EINTR.
        Err(Errno::EINTR.into())
    }
}
