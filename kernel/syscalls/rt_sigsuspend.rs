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

        // Save old mask, install temporary mask.
        let old_mask = current.sigset_load();
        current.sigset_store(new_mask);

        // Stash old_mask for rt_sigreturn to restore after the handler runs.
        // We must NOT restore it here — the temporary mask must remain active
        // so try_delivering_signal can deliver the pending signal.
        current.sigsuspend_save_mask(old_mask);

        // Block until a signal arrives.
        crate::poll::POLL_WAIT_QUEUE.sleep_signalable_until(|| Ok(None))?;

        // sigsuspend always returns EINTR.
        Err(Errno::EINTR.into())
    }
}
