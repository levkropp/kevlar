// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// membarrier(2) — issue cross-CPU memory barriers from user space.
//
// Provenance: Own (Linux membarrier(2) man page; Linux source
// kernel/sched/membarrier.c).
//
// Userspace uses this to synchronise lock-free data structures shared
// between threads on different CPUs without paying for a full barrier
// in the fast path.  Glibc/musl, Xorg, and JIT compilers (V8, JVM)
// all use it; on arm64 in particular Xorg's signal-handler /
// MIT-SHM accounting ends up here.  Stubbing it as no-op `Ok(0)` is
// observably wrong: prior stores on the originating CPU may not be
// visible to user code on other CPUs after the syscall returns.

use crate::result::{Errno, Result};
use crate::syscalls::SyscallHandler;

/// MEMBARRIER_CMD_QUERY — return a bitmask of supported commands.
const MEMBARRIER_CMD_QUERY: i32 = 0;
/// MEMBARRIER_CMD_GLOBAL — full system memory barrier on all CPUs.
const MEMBARRIER_CMD_GLOBAL: i32 = 1;
/// MEMBARRIER_CMD_GLOBAL_EXPEDITED — same, faster on systems that
/// register intent.  We treat it identically to GLOBAL.
const MEMBARRIER_CMD_GLOBAL_EXPEDITED: i32 = 2;
const MEMBARRIER_CMD_REGISTER_GLOBAL_EXPEDITED: i32 = 4;
/// MEMBARRIER_CMD_PRIVATE_EXPEDITED — barrier on threads of the
/// calling process.  We don't track thread sets per-process yet, so
/// we conservatively broadcast to all CPUs (correct, just heavier).
const MEMBARRIER_CMD_PRIVATE_EXPEDITED: i32 = 8;
const MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED: i32 = 16;

/// Bitmask of commands we report supported via QUERY.
const SUPPORTED_MASK: isize = (1 << MEMBARRIER_CMD_GLOBAL) as isize
    | (1 << MEMBARRIER_CMD_GLOBAL_EXPEDITED) as isize
    | (1 << MEMBARRIER_CMD_REGISTER_GLOBAL_EXPEDITED) as isize
    | (1 << MEMBARRIER_CMD_PRIVATE_EXPEDITED) as isize
    | (1 << MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED) as isize;

impl<'a> SyscallHandler<'a> {
    pub fn sys_membarrier(&mut self, cmd: i32, _flags: u32, _cpu_id: i32) -> Result<isize> {
        match cmd {
            MEMBARRIER_CMD_QUERY => Ok(SUPPORTED_MASK),
            MEMBARRIER_CMD_REGISTER_GLOBAL_EXPEDITED
            | MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED => {
                // Registration is a per-process opt-in; we have nothing
                // process-scoped to record, so accept silently.
                Ok(0)
            }
            MEMBARRIER_CMD_GLOBAL
            | MEMBARRIER_CMD_GLOBAL_EXPEDITED
            | MEMBARRIER_CMD_PRIVATE_EXPEDITED => {
                // Local barrier on the originating CPU, then broadcast
                // an SGI/IPI to every other CPU; the receiver issues
                // the matching barrier in the IRQ handler before
                // returning to user space.
                kevlar_platform::arch::local_memory_barrier();
                kevlar_platform::arch::broadcast_membarrier_ipi();
                Ok(0)
            }
            _ => Err(Errno::EINVAL.into()),
        }
    }
}
