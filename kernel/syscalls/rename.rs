// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
// Own implementation based on Linux man pages.
use crate::fs::{inotify, path::Path};
use crate::prelude::*;
use crate::{process::current_process, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_rename(&mut self, old_path: &Path, new_path: &Path) -> Result<isize> {
        let (old_parent, old_name) = old_path
            .parent_and_basename()
            .ok_or_else::<Error, _>(|| Errno::ENOENT.into())?;
        let (new_parent, new_name) = new_path
            .parent_and_basename()
            .ok_or_else::<Error, _>(|| Errno::ENOENT.into())?;

        let root_fs_arc = current_process().root_fs();
        let root_fs = root_fs_arc.lock();
        let old_parent_dir = root_fs.lookup_dir(old_parent)?;
        let new_parent_dir = root_fs.lookup_dir(new_parent)?;
        old_parent_dir.rename(old_name, &new_parent_dir, new_name)?;
        drop(root_fs);
        inotify::notify_rename(old_parent.as_str(), old_name, new_parent.as_str(), new_name);
        Ok(0)
    }
}
