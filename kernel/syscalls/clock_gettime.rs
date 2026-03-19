// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use kevlar_platform::address::UserVAddr;

use crate::result::{Errno, Result};
use crate::{
    ctypes::{c_clockid, c_long, c_time, CLOCK_BOOTTIME, CLOCK_BOOTTIME_ALARM,
             CLOCK_MONOTONIC, CLOCK_MONOTONIC_COARSE, CLOCK_MONOTONIC_RAW,
             CLOCK_PROCESS_CPUTIME_ID, CLOCK_REALTIME, CLOCK_REALTIME_ALARM,
             CLOCK_REALTIME_COARSE, CLOCK_TAI, CLOCK_THREAD_CPUTIME_ID},
    timer::read_wall_clock,
};
use crate::{syscalls::SyscallHandler, timer::read_monotonic_clock};
use core::mem::size_of;

/// Packed timespec written in a single usercopy operation.
#[repr(C)]
#[derive(Clone, Copy)]
struct Timespec {
    tv_sec: c_time,
    tv_nsec: c_long,
}

impl<'a> SyscallHandler<'a> {
    pub fn sys_clock_gettime(&mut self, clock: c_clockid, buf: UserVAddr) -> Result<isize> {
        let total_ns = match clock {
            CLOCK_REALTIME | CLOCK_REALTIME_COARSE | CLOCK_REALTIME_ALARM | CLOCK_TAI =>
                read_wall_clock().nanosecs_from_epoch(),
            CLOCK_MONOTONIC | CLOCK_MONOTONIC_RAW | CLOCK_MONOTONIC_COARSE |
            CLOCK_BOOTTIME | CLOCK_BOOTTIME_ALARM |
            CLOCK_PROCESS_CPUTIME_ID | CLOCK_THREAD_CPUTIME_ID =>
                read_monotonic_clock().nanosecs(),
            _ => {
                debug_warn!("clock_gettime: unsupported clock id: {}", clock);
                return Err(Errno::ENOSYS.into());
            }
        };

        let ts = Timespec {
            tv_sec: (total_ns / 1_000_000_000) as c_time,
            tv_nsec: (total_ns % 1_000_000_000) as c_long,
        };

        // Single 16-byte usercopy instead of two separate 8-byte writes.
        let bytes = kevlar_platform::pod::copy_as_bytes(&ts);
        debug_assert_eq!(bytes.len(), size_of::<c_time>() + size_of::<c_long>());
        buf.write_bytes(bytes)?;

        Ok(0)
    }

    pub fn sys_clock_getres(&mut self, clock: c_clockid, res: Option<UserVAddr>) -> Result<isize> {
        // Validate clock ID.
        match clock {
            CLOCK_REALTIME | CLOCK_REALTIME_COARSE | CLOCK_REALTIME_ALARM | CLOCK_TAI |
            CLOCK_MONOTONIC | CLOCK_MONOTONIC_RAW | CLOCK_MONOTONIC_COARSE |
            CLOCK_BOOTTIME | CLOCK_BOOTTIME_ALARM |
            CLOCK_PROCESS_CPUTIME_ID | CLOCK_THREAD_CPUTIME_ID => {}
            _ => return Err(Errno::EINVAL.into()),
        }

        if let Some(buf) = res {
            // Report 1ns resolution (TSC-based clock).
            let ts = Timespec { tv_sec: 0, tv_nsec: 1 };
            let bytes = kevlar_platform::pod::copy_as_bytes(&ts);
            buf.write_bytes(bytes)?;
        }

        Ok(0)
    }
}
