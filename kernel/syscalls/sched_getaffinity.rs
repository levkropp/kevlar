// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// sched_getaffinity(pid, cpusetsize, mask)
//
// Stub: reports CPU 0 as the only available CPU.  The cpu_set_t is a plain
// bitmask; setting bit 0 in byte 0 means "CPU 0 is available".
use crate::{
    ctypes::c_int,
    result::{Errno, Result},
    syscalls::SyscallHandler,
};
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    pub fn sys_sched_getaffinity(
        &mut self,
        _pid: c_int,
        cpusetsize: usize,
        mask: UserVAddr,
    ) -> Result<isize> {
        if cpusetsize == 0 {
            return Err(Errno::EINVAL.into());
        }
        let size = cpusetsize.min(128);
        let mut buf = [0u8; 128];
        buf[0] = 0x01; // CPU 0 available
        mask.write_bytes(&buf[..size])?;
        Ok(0)
    }
}
