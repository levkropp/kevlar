// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
// Own implementation based on Linux man pages.
use crate::ctypes::{c_long, c_time};
use crate::prelude::*;
use crate::syscalls::SyscallHandler;
use crate::timer::_sleep_ms;
use kevlar_platform::address::UserVAddr;

#[repr(C)]
struct Timespec {
    tv_sec: c_time,
    tv_nsec: c_long,
}

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
}
