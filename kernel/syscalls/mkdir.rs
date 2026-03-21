// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::fs::{inotify, path::Path, stat::FileMode};
use crate::prelude::*;
use crate::{process::current_process, syscalls::SyscallHandler};
use kevlar_vfs::stat::{GId, UId};

impl<'a> SyscallHandler<'a> {
    pub fn sys_mkdir(&mut self, path: &Path, mode: FileMode) -> Result<isize> {
        let (parent_dir, name) = path
            .parent_and_basename()
            .ok_or_else::<Error, _>(|| Errno::EEXIST.into())?;

        let current = current_process();
        let effective_mode = FileMode::new(mode.as_u32() & !current.umask());
        let root_fs = current.root_fs();
        root_fs
            .lock()
            .lookup_dir(parent_dir)?
            .create_dir(name, effective_mode, UId::new(current.euid()), GId::new(current.egid()))?;

        inotify::notify(parent_dir.as_str(), name, inotify::IN_CREATE);
        Ok(0)
    }
}
