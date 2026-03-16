// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
// Own implementation based on Linux man pages.
use crate::fs::inotify;
use crate::fs::path::Path;
use crate::prelude::*;
use crate::{process::current_process, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_unlink(&mut self, path: &Path) -> Result<isize> {
        let (parent, name) = path
            .parent_and_basename()
            .ok_or_else::<Error, _>(|| Errno::ENOENT.into())?;

        let root_fs_arc = current_process().root_fs();
        let root_fs = root_fs_arc.lock();
        let parent_dir = root_fs.lookup_dir(parent)?;
        parent_dir.unlink(name)?;
        drop(root_fs);
        inotify::notify(parent.as_str(), name, inotify::IN_DELETE);
        Ok(0)
    }
}
