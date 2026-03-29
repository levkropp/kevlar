// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
// Own implementation based on Linux man pages.
use crate::ctypes::c_int;
use crate::prelude::*;
use crate::process::current_process;
use crate::syscalls::SyscallHandler;
use crate::user_buffer::UserBufWriter;
use core::mem::size_of;
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    pub fn sys_getrlimit(&mut self, resource: c_int, buf: UserVAddr) -> Result<isize> {
        let current = current_process();
        let rlimits = current.rlimits();
        let idx = resource as usize;
        let (cur, max) = if idx < 16 {
            (rlimits[idx][0], rlimits[idx][1])
        } else {
            (!0u64, !0u64) // Unknown resources: return INFINITY
        };

        let mut writer = UserBufWriter::from_uaddr(buf, 2 * size_of::<u64>());
        writer.write::<u64>(cur)?;
        writer.write::<u64>(max)?;
        Ok(0)
    }

    pub fn sys_setrlimit(&mut self, resource: c_int, buf: UserVAddr) -> Result<isize> {
        let new_cur: u64 = buf.read()?;
        let off8 = UserVAddr::new(buf.as_isize() as usize + 8).ok_or_else(|| crate::result::Error::new(crate::result::Errno::EFAULT))?;
        let new_max: u64 = off8.read()?;

        let current = current_process();
        current.set_rlimit(resource as usize, new_cur, new_max);
        Ok(0)
    }
}
