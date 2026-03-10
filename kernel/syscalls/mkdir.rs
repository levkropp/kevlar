// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::fs::{inotify, path::Path, stat::FileMode};
use crate::prelude::*;
use crate::{process::current_process, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_mkdir(&mut self, path: &Path, mode: FileMode) -> Result<isize> {
        let (parent_dir, name) = path
            .parent_and_basename()
            .ok_or_else::<Error, _>(|| Errno::EEXIST.into())?;

        current_process()
            .root_fs()
            .lock()
            .lookup_dir(parent_dir)?
            .create_dir(name, mode)?;

        inotify::notify(parent_dir.as_str(), name, inotify::IN_CREATE);
        Ok(0)
    }
}
