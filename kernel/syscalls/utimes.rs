// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use kevlar_platform::address::UserVAddr;

use crate::fs::path::Path;
use crate::prelude::*;
use crate::timer::read_wall_clock;
use crate::{process::current_process, syscalls::SyscallHandler};

/// Userspace `struct timeval` (16 bytes on x86_64/aarch64).
#[derive(Debug, Copy, Clone)]
#[repr(C)]
struct Timeval {
    tv_sec: isize,
    tv_usec: isize,
}

impl<'a> SyscallHandler<'a> {
    pub fn sys_utimes(&mut self, path: &Path, times: Option<UserVAddr>) -> Result<isize> {
        let root_fs = current_process().root_fs();
        let inode = root_fs.lock().lookup(path)?;

        let (atime_secs, mtime_secs) = if let Some(tp) = times {
            let tv: [Timeval; 2] = tp.read()?;
            (Some(tv[0].tv_sec), Some(tv[1].tv_sec))
        } else {
            // times == NULL: set both to current time.
            let now = read_wall_clock().secs_from_epoch() as isize;
            (Some(now), Some(now))
        };

        inode.set_times(atime_secs, mtime_secs)?;
        Ok(0)
    }
}
