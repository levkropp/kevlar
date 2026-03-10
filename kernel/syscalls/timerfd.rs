// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! timerfd_create(2), timerfd_settime(2) syscall handlers.
//!
//! Provenance: Own (Linux timerfd_create(2), timerfd_settime(2) man pages).
use crate::{
    ctypes::c_int,
    fs::{
        inode::{FileLike, INode},
        opened_file::{Fd, OpenOptions, PathComponent},
        timerfd::{CLOCK_MONOTONIC, CLOCK_REALTIME, TFD_CLOEXEC, TFD_NONBLOCK, TimerFd},
    },
    prelude::*,
    process::current_process,
    syscalls::SyscallHandler,
};
use kevlar_platform::address::UserVAddr;
use kevlar_utils::downcast::Downcastable;

/// `struct itimerspec` layout: two `struct timespec` (sec: i64, nsec: i64).
/// Total: 32 bytes.
const ITIMERSPEC_SIZE: usize = 32;

impl<'a> SyscallHandler<'a> {
    /// `timerfd_create(clockid, flags)` — create a timer fd.
    pub fn sys_timerfd_create(&mut self, clockid: c_int, flags: c_int) -> Result<isize> {
        match clockid {
            CLOCK_REALTIME | CLOCK_MONOTONIC => {}
            _ => return Err(Errno::EINVAL.into()),
        }

        let cloexec = (flags & TFD_CLOEXEC) != 0;
        let nonblock = (flags & TFD_NONBLOCK) != 0;
        let options = OpenOptions::new(nonblock, cloexec);

        let tfd = TimerFd::new();
        let fd = current_process().opened_files().lock().open(
            PathComponent::new_anonymous(INode::FileLike(tfd as Arc<dyn FileLike>)),
            options,
        )?;
        Ok(fd.as_int() as isize)
    }

    /// `timerfd_settime(fd, flags, new_value, old_value)` — arm/disarm a timer.
    pub fn sys_timerfd_settime(
        &mut self,
        fd: Fd,
        _flags: c_int,
        new_value_ptr: UserVAddr,
    ) -> Result<isize> {
        // Read struct itimerspec from userspace.
        let bytes = new_value_ptr.read::<[u8; ITIMERSPEC_SIZE]>()?;

        // Parse itimerspec: { it_interval: timespec, it_value: timespec }
        // timespec = { tv_sec: i64, tv_nsec: i64 }
        let interval_sec = i64::from_ne_bytes(bytes[0..8].try_into().unwrap());
        let interval_nsec = i64::from_ne_bytes(bytes[8..16].try_into().unwrap());
        let value_sec = i64::from_ne_bytes(bytes[16..24].try_into().unwrap());
        let value_nsec = i64::from_ne_bytes(bytes[24..32].try_into().unwrap());

        // Get the TimerFd from the fd table.
        let table = current_process().opened_files().lock();
        let file = table.get(fd)?.as_file()?;
        let timer = file.as_any().downcast_ref::<TimerFd>()
            .ok_or(Error::new(Errno::EINVAL))?;

        timer.settime(value_sec, value_nsec, interval_sec, interval_nsec);
        Ok(0)
    }
}
