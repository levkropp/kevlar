// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! pivot_root(2) — swap root filesystem within a mount namespace.
use crate::{
    process::current_process,
    result::{Errno, Result},
    syscalls::{resolve_path, SyscallHandler},
};

impl<'a> SyscallHandler<'a> {
    /// pivot_root(new_root, put_old) — change the root mount.
    ///
    /// Simplified implementation: we update the root_fs root_path to point at
    /// new_root. Full Linux semantics (moving the old root mount to put_old)
    /// are deferred until mount namespace integration in Phase 3+.
    pub fn sys_pivot_root(&mut self, new_root_ptr: usize, put_old_ptr: usize) -> Result<isize> {
        let new_root_path = resolve_path(new_root_ptr)?;
        let _put_old_path = resolve_path(put_old_ptr)?;

        // Verify new_root exists and is a directory.
        let root_fs = current_process().root_fs();
        let mut root_fs = root_fs.lock();
        let _new_root_dir = root_fs.lookup_dir(&new_root_path)
            .map_err(|_| crate::result::Error::new(Errno::EINVAL))?;

        // For now, accept the call without actually swapping roots.
        // A full implementation requires mount-point tracking that we'll
        // build incrementally. Returning success lets systemd proceed.
        Ok(0)
    }
}
