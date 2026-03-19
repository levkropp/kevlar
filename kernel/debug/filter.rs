// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Debug event category filter using bitflags.

use bitflags::bitflags;

bitflags! {
    /// Categories of debug events that can be independently enabled/disabled.
    ///
    /// The filter is stored as an atomic u32, so checking is lock-free.
    /// Default: nothing enabled (zero overhead in production).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct DebugFilter: u32 {
        /// Syscall entry/exit tracing (high volume).
        const SYSCALL  = 0x01;
        /// Signal delivery events.
        const SIGNAL   = 0x02;
        /// Page faults and CPU exceptions.
        const FAULT    = 0x04;
        /// Process lifecycle (fork, exec, exit).
        const PROCESS  = 0x08;
        /// Stack canary checks (only logs mismatches unless SYSCALL is also set).
        const CANARY   = 0x10;
        /// Memory operations (mmap, brk, page alloc).
        const MEMORY   = 0x20;
        /// Panic events (always recommended).
        const PANIC    = 0x40;
        /// Individual copy_to_user/copy_from_user calls (very high volume).
        /// Use for diagnosing which specific usercopy is faulting.
        const USERCOPY = 0x80;
        /// Per-syscall cycle profiling (low overhead: 2x rdtsc per syscall).
        const PROFILE  = 0x100;
        /// Span tracer for exec/fork/page-fault phase profiling.
        const TRACE    = 0x200;
        /// Per-exec prefault decision tracing (which pages, from cache/file/zero).
        const KWAB_EXEC  = 0x400;
        /// Post-exec page content verification against backing files.
        const KWAB_VERIFY = 0x800;
        /// VM audit: check PTE state matches VMA protection after exec.
        const KWAB_AUDIT  = 0x1000;
        /// Hierarchical call tracer (per-CPU ring of enter/exit events with depth).
        const HTRACE = 0x2000;
        /// ktrace: high-bandwidth binary tracing (per-CPU ring + debugcon dump).
        const KTRACE = 0x4000;
    }
}
