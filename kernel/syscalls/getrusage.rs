// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX getrusage(2) man page).
use crate::ctypes::*;
use crate::result::Result;
use crate::syscalls::SyscallHandler;
use kevlar_runtime::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    pub fn sys_getrusage(&mut self, _who: c_int, usage: UserVAddr) -> Result<isize> {
        // Stub: zero-fill the rusage struct (144 bytes on x86_64).
        let zeros = [0u8; 144];
        usage.write_bytes(&zeros)?;
        Ok(0)
    }
}
