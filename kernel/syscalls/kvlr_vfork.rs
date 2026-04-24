// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! `kvlr_vfork()` — Linux-vfork-compatible primitive on the Kevlar-private
//! SYS_KVLR_VFORK = 501 namespace.
//!
//! Implementation: `Process::vfork` (the existing one used by sys_vfork)
//! shares the parent's VM Arc with the child — no page-table duplication
//! whatsoever.  Parent blocks on `VFORK_WAIT_QUEUE` until the child calls
//! `_exit` or `execve`.  This is the Linux vfork(2) model exactly: parent
//! suspended, child runs in parent's address space, child must call
//! `_exit` or one of the `exec*` family before touching any parent-visible
//! state.
//!
//! Why not ghost-fork CoW?  Initial design used `Process::fork_ghost`
//! (ghost-fork with CoW PTEs) for "safer than Linux vfork" semantics.
//! Profile revealed `fork.ghost` avg 22.3 µs/call on arm64 HVF — 3.4 ×
//! slower than regular fork's page_table work, because our
//! `duplicate_table_ghost` allocates a fresh PT at every level (regular
//! fork's `share_leaf_pt` shares leaf PTs without allocation).  Ghost-fork
//! is a net loss in our implementation.
//!
//! Since the parent is *guaranteed* blocked (the syscall itself doesn't
//! return to userspace until child exit/exec), there's no concurrent-write
//! window to protect.  Plain VM sharing is both faster and sufficient.
//!
//! Why expose a new syscall number at all when `sys_vfork` (SYS_VFORK = 1071
//! on arm64) already does this?  Two reasons: (1) Kevlar-private namespace
//! (500-series) lets us add kvlr-specific flags later without touching
//! Linux ABI, (2) bench.c can probe SYS_KVLR_VFORK → ENOSYS on Linux and
//! BENCH_SKIP cleanly, whereas SYS_VFORK on Linux succeeds and measures
//! Linux's vfork — a legitimate comparison but different from what we
//! want in the spawn-style "Kevlar-only primitive" section of the report.
//!
//! Target workload: `fork + _exit + wait` pattern.  Saves the entire
//! `duplicate_from` call (~6 µs) + one context-switch round-trip
//! (~3 µs) vs regular `fork()` + `wait()`.  See blog 226.

use crate::process::{current_process, signal::SigSet, Process, VFORK_WAIT_QUEUE};
use crate::result::Result;
use crate::syscalls::SyscallHandler;
use core::sync::atomic::Ordering;

impl<'a> SyscallHandler<'a> {
    pub fn sys_kvlr_vfork(&mut self) -> Result<isize> {
        // Process::vfork shares the parent's VM Arc — no page-table copy.
        // Safe because the parent is about to block in VFORK_WAIT_QUEUE
        // below and won't resume until the child releases the VM via
        // _exit or execve.
        let child = Process::vfork(current_process(), self.frame)?;
        let child_pid = child.pid().as_i32() as isize;
        let current = current_process();

        // Block all signals for the vfork wait — signals remain queued
        // and deliver after the syscall returns.  Same pattern as
        // sys_vfork and the CLONE_VFORK branch of sys_clone.
        let saved_mask = current.sigset_load();
        current.sigset_store(SigSet::ALL);

        while !child.ghost_fork_done.load(Ordering::Acquire) {
            let _ = VFORK_WAIT_QUEUE.sleep_signalable_until(|| {
                if child.ghost_fork_done.load(Ordering::Acquire) {
                    Ok(Some(()))
                } else {
                    Ok(None)
                }
            });
        }

        current.sigset_store(saved_mask);
        Ok(child_pid)
    }
}
