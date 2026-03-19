// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::process::current_process;
use crate::result::Result;
use crate::syscalls::SyscallHandler;
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    pub fn sys_uname(&mut self, buf: UserVAddr) -> Result<isize> {
        let utsname = current_process().utsname_copy();
        buf.write_bytes(&utsname)?;
        Ok(0)
    }
}
