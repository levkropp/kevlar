// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// clone(flags, child_stack, ptid, ctid, newtls)
//   x86_64: clone(rdi=flags, rsi=child_stack, rdx=ptid, r10=ctid, r8=newtls)
//   ARM64:  clone(x0=flags, x1=child_stack, x2=ptid, x3=newtls, x4=ctid)

use core::sync::atomic::Ordering;

use crate::{
    process::{current_process, signal::SigSet, Process, VFORK_WAIT_QUEUE},
    result::{Errno, Result},
    syscalls::SyscallHandler,
};
use kevlar_platform::address::UserVAddr;

const CLONE_VM: usize        = 0x00000100;
#[allow(dead_code)]
const CLONE_FS: usize        = 0x00000200;
const CLONE_FILES: usize     = 0x00000400;
#[allow(dead_code)]
const CLONE_SIGHAND: usize   = 0x00000800;
const CLONE_VFORK: usize     = 0x00004000;
#[allow(dead_code)]
const CLONE_THREAD: usize    = 0x00010000;
const CLONE_CHILD_SETTID: usize  = 0x01000000;
const CLONE_CHILD_CLEARTID: usize = 0x00200000;
const CLONE_PARENT_SETTID: usize = 0x00100000;
const CLONE_SETTLS: usize    = 0x00080000;
#[allow(dead_code)]
const CSIGNAL_MASK: usize    = 0xff;

impl<'a> SyscallHandler<'a> {
    pub fn sys_clone(
        &mut self,
        flags: usize,
        child_stack: usize,
        ptid: usize,
        ctid_or_newtls: usize,
        newtls_or_ctid: usize,
    ) -> Result<isize> {
        // x86_64: (flags, child_stack, ptid, ctid, newtls)
        // ARM64:  (flags, child_stack, ptid, newtls, ctid)
        // We want: ctid = address in child's address space, newtls = TLS base
        #[cfg(target_arch = "x86_64")]
        let (ctid, newtls) = (ctid_or_newtls, newtls_or_ctid);
        #[cfg(target_arch = "aarch64")]
        let (newtls, ctid) = (ctid_or_newtls, newtls_or_ctid);

        let parent = current_process();

        if flags & CLONE_VM != 0 {
            // Shared VM: thread or posix_spawn-style clone.
            if child_stack == 0 {
                return Err(Errno::EINVAL.into());
            }

            let set_child_tid   = flags & CLONE_CHILD_SETTID  != 0;
            let clear_child_tid = flags & CLONE_CHILD_CLEARTID != 0;
            let newtls_val = if flags & CLONE_SETTLS != 0 { newtls as u64 } else { 0 };
            let is_vfork = flags & CLONE_VFORK != 0;
            let is_thread = flags & CLONE_THREAD != 0;

            let child = Process::new_thread(
                parent,
                self.frame,
                child_stack as u64,
                newtls_val,
                ctid,
                set_child_tid,
                clear_child_tid,
                is_vfork,
                is_thread,
            )?;

            if flags & CLONE_PARENT_SETTID != 0 && ptid != 0 {
                if let Ok(uaddr) = UserVAddr::new_nonnull(ptid) {
                    let _ = uaddr.write::<i32>(&child.pid().as_i32());
                }
            }

            let child_pid = child.pid().as_i32() as isize;

            // CLONE_VFORK: block the parent until the child execs or exits.
            // The child's execve/exit calls wake_vfork_parent() which sets
            // ghost_fork_done and wakes VFORK_WAIT_QUEUE.
            if is_vfork {
                let saved_mask = parent.sigset_load();
                parent.sigset_store(SigSet::ALL);
                while !child.ghost_fork_done.load(Ordering::Acquire) {
                    let _ = VFORK_WAIT_QUEUE.sleep_signalable_until(|| {
                        if child.ghost_fork_done.load(Ordering::Acquire) {
                            Ok(Some(()))
                        } else {
                            Ok(None)
                        }
                    });
                }
                parent.sigset_store(saved_mask);
            }

            Ok(child_pid)
        } else {
            // Fork: copy address space.
            if child_stack != 0 {
                debug_warn!("clone: non-zero child_stack without CLONE_VM, ignoring");
            }

            // Handle namespace flags (CLONE_NEWUTS, CLONE_NEWPID, CLONE_NEWNS).
            let ns_flags = flags & (crate::namespace::CLONE_NEWUTS
                | crate::namespace::CLONE_NEWPID
                | crate::namespace::CLONE_NEWNS
                | crate::namespace::CLONE_NEWNET);

            if flags & crate::namespace::CLONE_NEWNET != 0 {
                return Err(Errno::EINVAL.into());
            }

            let child = Process::fork(parent, self.frame)?;

            // Apply namespace changes to the child if any CLONE_NEW* flags are set.
            if ns_flags != 0 {
                let parent_ns = parent.namespaces();
                let child_ns = parent_ns.clone_with_flags(ns_flags)?;

                // For CLONE_NEWPID, allocate a namespace-local PID for the child.
                if ns_flags & crate::namespace::CLONE_NEWPID != 0 {
                    let ns_pid = child_ns.pid_ns.alloc_ns_pid(child.pid());
                    // ns_pid is immutable, so we set it via an interior method.
                    child.set_ns_pid(ns_pid);
                }

                child.set_namespaces(child_ns);
            }

            // Handle SETTID/CLEARTID for fork-like clones.
            if flags & CLONE_CHILD_SETTID != 0 && ctid != 0 {
                if let Ok(uaddr) = UserVAddr::new_nonnull(ctid) {
                    let _ = uaddr.write::<i32>(&child.pid().as_i32());
                }
            }
            if flags & CLONE_CHILD_CLEARTID != 0 && ctid != 0 {
                child.set_clear_child_tid(ctid);
            }
            if flags & CLONE_PARENT_SETTID != 0 && ptid != 0 {
                if let Ok(uaddr) = UserVAddr::new_nonnull(ptid) {
                    let _ = uaddr.write::<i32>(&child.pid().as_i32());
                }
            }

            Ok(child.pid().as_i32() as isize)
        }
    }
}
