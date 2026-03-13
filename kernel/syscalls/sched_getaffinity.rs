// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// sched_getaffinity(pid, cpusetsize, mask)
//
// Returns a cpu_set_t bitmask with one bit set per online CPU.  musl uses
// this for sysconf(_SC_NPROCESSORS_ONLN), so returning only CPU 0 would
// make every process believe it's on a 1-CPU machine even under `-smp 4`.
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
        let num_cpus = kevlar_platform::arch::num_online_cpus() as usize;
        // cpu_set_t is a byte array; bit i lives in byte[i/8] at position i%8.
        let size = cpusetsize.min(128);
        let mut buf = [0u8; 128];
        for cpu in 0..num_cpus.min(size * 8) {
            buf[cpu / 8] |= 1 << (cpu % 8);
        }
        mask.write_bytes(&buf[..size])?;
        Ok(0)
    }
}
