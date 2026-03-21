// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! pivot_root(2) — swap root filesystem.
//!
//! Makes new_root the new `/` for the calling process's mount namespace.
//! The old root is moved to put_old (which must be a directory under new_root).
use crate::{
    process::current_process,
    result::{Errno, Result},
    syscalls::{resolve_path, SyscallHandler},
};

impl<'a> SyscallHandler<'a> {
    /// pivot_root(new_root, put_old) — change the root mount.
    ///
    /// Simplified implementation: looks up the filesystem mounted at new_root
    /// and makes it the new root filesystem for the calling process. The old
    /// root is not actually moved to put_old (deferred until full mount
    /// namespace support). This is sufficient for init systems that
    /// pivot_root + umount the old root immediately after.
    pub fn sys_pivot_root(&mut self, new_root_ptr: usize, put_old_ptr: usize) -> Result<isize> {
        let new_root_path = resolve_path(new_root_ptr)?;
        let _put_old_path = resolve_path(put_old_ptr)?;

        let current = current_process();
        let root_fs_arc = current.root_fs();
        let mut root_fs = root_fs_arc.lock();

        // Look up the directory at new_root.
        let new_root_dir = root_fs.lookup_dir(&new_root_path)
            .map_err(|_| crate::result::Error::new(Errno::EINVAL))?;

        // Check if there's a filesystem mounted at new_root. If so, use its
        // root directory as the new root. If not, use new_root_dir directly.
        if let Some(mounted_fs) = root_fs.get_mount_at_dir(&new_root_dir) {
            let new_root = mounted_fs.root_dir()
                .map_err(|_| crate::result::Error::new(Errno::EINVAL))?;
            root_fs.set_root(new_root);
        } else {
            root_fs.set_root(new_root_dir);
        }

        // Reset cwd to "/" in the new root.
        root_fs.set_cwd("/");

        Ok(0)
    }
}
