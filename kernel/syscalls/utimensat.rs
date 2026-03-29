// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (Linux utimensat(2) man page).
use crate::prelude::*;
use crate::process::current_process;
use crate::syscalls::{CwdOrFd, SyscallHandler};
use crate::timer::read_wall_clock;
use kevlar_platform::address::UserVAddr;

const UTIME_NOW: isize = (1 << 30) - 1;
const UTIME_OMIT: isize = (1 << 30) - 2;
const AT_SYMLINK_NOFOLLOW: i32 = 0x100;

/// Userspace `struct timespec` layout (16 bytes on both x86_64 and aarch64).
#[derive(Debug, Copy, Clone)]
#[repr(C)]
struct Timespec {
    tv_sec: isize,
    tv_nsec: isize,
}

impl<'a> SyscallHandler<'a> {
    pub fn sys_utimensat(
        &mut self,
        dirfd: CwdOrFd,
        pathname: usize,
        times: Option<UserVAddr>,
        flags: i32,
    ) -> Result<isize> {
        let now_secs = read_wall_clock().secs_from_epoch() as isize;
        let follow_symlinks = (flags & AT_SYMLINK_NOFOLLOW) == 0;

        // Determine atime/mtime. None = UTIME_OMIT (don't change).
        let (atime_secs, mtime_secs) = if let Some(tp) = times {
            let ts: [Timespec; 2] = tp.read()?;
            let a = if ts[0].tv_nsec == UTIME_OMIT {
                None
            } else if ts[0].tv_nsec == UTIME_NOW {
                Some(now_secs)
            } else {
                Some(ts[0].tv_sec)
            };
            let m = if ts[1].tv_nsec == UTIME_OMIT {
                None
            } else if ts[1].tv_nsec == UTIME_NOW {
                Some(now_secs)
            } else {
                Some(ts[1].tv_sec)
            };
            (a, m)
        } else {
            // times == NULL: set both to current time.
            (Some(now_secs), Some(now_secs))
        };

        // If both are OMIT, nothing to do.
        if atime_secs.is_none() && mtime_secs.is_none() {
            return Ok(0);
        }

        // Resolve the target inode.
        let inode = if pathname == 0 {
            // pathname is NULL: operate on the fd itself (AT_EMPTY_PATH-like).
            match dirfd {
                CwdOrFd::Fd(fd) => {
                    current_process().with_file(fd, |opened_file| {
                        Ok(opened_file.path().inode.clone())
                    })?
                }
                CwdOrFd::AtCwd => return Err(Errno::EFAULT.into()),
            }
        } else {
            let uaddr = UserVAddr::new_nonnull(pathname)?;
            let user_cstr = super::UserCStr::new(uaddr, 512)?;
            let path = crate::fs::path::Path::new(user_cstr.as_str());
            let root_fs = current_process().root_fs();
            // Drop root_fs lock before calling set_times (which may write to disk).
            if follow_symlinks {
                root_fs.lock().lookup(&path)?
            } else {
                root_fs.lock().lookup_no_symlink_follow(&path)?
            }
        };

        inode.set_times(atime_secs, mtime_secs)?;
        Ok(0)
    }
}
