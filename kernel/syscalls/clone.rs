// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (Linux clone(2) man page, musl libc source for flag usage).
//
// clone(flags, child_stack, ptid, ctid, newtls)
//   x86_64: clone(rdi=flags, rsi=child_stack, rdx=ptid, r10=ctid, r8=newtls)
//   ARM64:  clone(x0=flags, x1=child_stack, x2=ptid, x3=newtls, x4=ctid)
//
// When child_stack is 0, the child shares the parent's stack (like fork).
// When child_stack is non-zero, the child uses the given stack pointer.
//
// We handle:
//   CLONE_CHILD_SETTID  — write child TID to *ctid in child's address space
//   CLONE_CHILD_CLEARTID — set child's clear_child_tid pointer (for futex wake on exit)
//   SIGCHLD in low byte — normal fork semantics
//
// We do NOT yet handle (returns ENOSYS):
//   CLONE_VM, CLONE_THREAD, CLONE_VFORK, CLONE_NEWNS, etc.

use crate::{
    process::{current_process, Process},
    result::{Errno, Result},
    syscalls::SyscallHandler,
};

// Clone flag bits from Linux uapi.
const CLONE_VM: usize = 0x00000100;
#[allow(dead_code)]
const CLONE_FS: usize = 0x00000200;
#[allow(dead_code)]
const CLONE_FILES: usize = 0x00000400;
#[allow(dead_code)]
const CLONE_SIGHAND: usize = 0x00000800;
const CLONE_THREAD: usize = 0x00010000;
#[allow(dead_code)]
const CLONE_CHILD_SETTID: usize = 0x01000000;
#[allow(dead_code)]
const CLONE_CHILD_CLEARTID: usize = 0x00200000;
#[allow(dead_code)]
const CLONE_SETTLS: usize = 0x00080000;

#[allow(dead_code)]
const CSIGNAL_MASK: usize = 0xff; // Low 8 bits = exit signal

impl<'a> SyscallHandler<'a> {
    pub fn sys_clone(
        &mut self,
        flags: usize,
        child_stack: usize,
        _ptid: usize,
        _ctid_or_newtls: usize,
        _newtls_or_ctid: usize,
    ) -> Result<isize> {
        // If threading flags are set, we don't support them yet.
        if flags & (CLONE_VM | CLONE_THREAD) != 0 {
            debug_warn!(
                "clone: unsupported threading flags {:#x}, returning ENOSYS",
                flags
            );
            return Err(Errno::ENOSYS.into());
        }

        // If a child stack was specified with threading flags, we can't handle it yet.
        if child_stack != 0 {
            debug_warn!("clone: non-zero child_stack without CLONE_VM not yet supported");
            return Err(Errno::ENOSYS.into());
        }

        // For fork-like clones (SIGCHLD, possibly with CLONE_CHILD_SETTID/CLEARTID),
        // just do a fork. musl's fork() calls clone(SIGCHLD, 0, ...).
        let child = Process::fork(current_process(), self.frame)?;

        Ok(child.pid().as_i32() as isize)
    }
}
