// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Reference: OSv core/osv_clock.cc (BSD-3-Clause) — gettimeofday.
// Returns wall clock as struct timeval {tv_sec, tv_usec}.
use crate::ctypes::{c_suseconds, c_time};
use crate::prelude::*;
use crate::syscalls::SyscallHandler;
use crate::timer::read_wall_clock;
use crate::user_buffer::UserBufWriter;
use core::mem::size_of;
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    pub fn sys_gettimeofday(&mut self, tv: UserVAddr) -> Result<isize> {
        let now = read_wall_clock();
        let secs = now.secs_from_epoch();
        let usecs = (now.msecs_from_epoch() % 1000) * 1000;

        let mut writer =
            UserBufWriter::from_uaddr(tv, size_of::<c_time>() + size_of::<c_suseconds>());
        writer.write::<c_time>(secs as c_time)?;
        writer.write::<c_suseconds>(usecs as c_suseconds)?;
        Ok(0)
    }
}
