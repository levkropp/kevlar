// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// sysinfo(2) — returns system statistics. We expose real page allocator
// stats for totalram/freeram and compute uptime from the monotonic clock.
use crate::ctypes::c_long;
use crate::prelude::*;
use crate::process::process_count;
use crate::syscalls::SyscallHandler;
use crate::timer::read_monotonic_clock;
use crate::user_buffer::UserBufWriter;
use kevlar_platform::arch::PAGE_SIZE;
use kevlar_platform::page_allocator::read_allocator_stats;
use kevlar_platform::address::UserVAddr;

/// Matches Linux's struct sysinfo layout on x86-64.
#[derive(Clone, Copy)]
#[repr(C)]
struct SysInfo {
    uptime: c_long,
    loads: [c_long; 3],
    totalram: u64,
    freeram: u64,
    sharedram: u64,
    bufferram: u64,
    totalswap: u64,
    freeswap: u64,
    procs: u16,
    pad: u16,
    pad2: u32,
    totalhigh: u64,
    freehigh: u64,
    mem_unit: u32,
    // padding to 64 bytes on x86-64
}

impl<'a> SyscallHandler<'a> {
    pub fn sys_sysinfo(&mut self, buf: UserVAddr) -> Result<isize> {
        let stats = read_allocator_stats();
        let uptime = read_monotonic_clock().secs() as c_long;
        let procs = process_count() as u16;

        let info = SysInfo {
            uptime,
            loads: [0; 3],
            totalram: (stats.num_total_pages * PAGE_SIZE) as u64,
            freeram: (stats.num_free_pages * PAGE_SIZE) as u64,
            sharedram: 0,
            bufferram: 0,
            totalswap: 0,
            freeswap: 0,
            procs,
            pad: 0,
            pad2: 0,
            totalhigh: 0,
            freehigh: 0,
            mem_unit: 1,
        };

        // Write as raw bytes.
        let bytes = kevlar_platform::pod::copy_as_bytes(&info);

        let mut writer = UserBufWriter::from_uaddr(buf, bytes.len());
        writer.write_bytes(bytes)?;
        Ok(0)
    }
}
