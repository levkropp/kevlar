// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
// Own implementation based on Linux man pages.
use crate::ctypes::{c_clockid, c_int, c_long, c_time};
use crate::prelude::*;
use crate::syscalls::SyscallHandler;
use crate::timer::_sleep_ms;
use kevlar_platform::address::UserVAddr;

#[repr(C)]
struct Timespec {
    tv_sec: c_time,
    tv_nsec: c_long,
}

const TIMER_ABSTIME: c_int = 1;

impl<'a> SyscallHandler<'a> {
    pub fn sys_nanosleep(&mut self, req: UserVAddr) -> Result<isize> {
        let ts = req.read::<Timespec>()?;

        if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 {
            return Err(Errno::EINVAL.into());
        }

        let ms = (ts.tv_sec as usize) * 1000 + (ts.tv_nsec as usize) / 1_000_000;
        if ms > 0 {
            _sleep_ms(ms);
        }

        Ok(0)
    }

    pub fn sys_clock_nanosleep(
        &mut self,
        clock: c_clockid,
        flags: c_int,
        req: UserVAddr,
        _rmtp: Option<UserVAddr>,
    ) -> Result<isize> {
        use crate::ctypes::{CLOCK_MONOTONIC, CLOCK_REALTIME, CLOCK_BOOTTIME,
                            CLOCK_MONOTONIC_COARSE, CLOCK_REALTIME_COARSE,
                            CLOCK_REALTIME_ALARM, CLOCK_BOOTTIME_ALARM, CLOCK_TAI};

        match clock {
            CLOCK_REALTIME | CLOCK_REALTIME_COARSE | CLOCK_REALTIME_ALARM | CLOCK_TAI |
            CLOCK_MONOTONIC | CLOCK_MONOTONIC_COARSE | CLOCK_BOOTTIME | CLOCK_BOOTTIME_ALARM => {}
            _ => return Err(Errno::EINVAL.into()),
        }

        let ts = req.read::<Timespec>()?;
        if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 {
            return Err(Errno::EINVAL.into());
        }

        let ms = if flags & TIMER_ABSTIME != 0 {
            // Absolute: compute delta from current monotonic clock.
            let target_ns = (ts.tv_sec as u64) * 1_000_000_000 + (ts.tv_nsec as u64);
            let now_ns = crate::timer::read_monotonic_clock().nanosecs() as u64;
            if target_ns <= now_ns {
                return Ok(0);
            }
            ((target_ns - now_ns) / 1_000_000) as usize
        } else {
            // Relative: sleep for the given duration.
            (ts.tv_sec as usize) * 1000 + (ts.tv_nsec as usize) / 1_000_000
        };

        if ms > 0 {
            _sleep_ms(ms);
        }

        Ok(0)
    }
}
