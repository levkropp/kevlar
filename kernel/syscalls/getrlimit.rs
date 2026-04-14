// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
// Own implementation based on Linux man pages.
use crate::ctypes::c_int;
use crate::prelude::*;
use crate::process::current_process;
use crate::syscalls::SyscallHandler;
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    pub fn sys_getrlimit(&mut self, resource: c_int, buf: UserVAddr) -> Result<isize> {
        let current = current_process();
        let pair = current.rlimit(resource as usize);
        // Write both cur and max in a single usercopy.
        buf.write(&pair)?;
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
