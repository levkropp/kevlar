// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// clone(flags, child_stack, ptid, ctid, newtls)
//   x86_64: clone(rdi=flags, rsi=child_stack, rdx=ptid, r10=ctid, r8=newtls)
//   ARM64:  clone(x0=flags, x1=child_stack, x2=ptid, x3=newtls, x4=ctid)

use crate::{
    process::{current_process, Process},
    result::{Errno, Result},
    syscalls::SyscallHandler,
};
use kevlar_platform::address::UserVAddr;

const CLONE_VM: usize        = 0x00000100;
#[allow(dead_code)]
const CLONE_FS: usize        = 0x00000200;
#[allow(dead_code)]
const CLONE_FILES: usize     = 0x00000400;
#[allow(dead_code)]
const CLONE_SIGHAND: usize   = 0x00000800;
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
            // Thread creation: share address space.
            if child_stack == 0 {
                return Err(Errno::EINVAL.into());
            }

            let set_child_tid   = flags & CLONE_CHILD_SETTID  != 0;
            let clear_child_tid = flags & CLONE_CHILD_CLEARTID != 0;
            let newtls_val = if flags & CLONE_SETTLS != 0 { newtls as u64 } else { 0 };

            let child = Process::new_thread(
                parent,
                self.frame,
                child_stack as u64,
                newtls_val,
                ctid,
                set_child_tid,
                clear_child_tid,
            )?;

            if flags & CLONE_PARENT_SETTID != 0 && ptid != 0 {
                if let Ok(uaddr) = UserVAddr::new_nonnull(ptid) {
                    let _ = uaddr.write::<i32>(&child.pid().as_i32());
                }
            }

            Ok(child.pid().as_i32() as isize)
        } else {
            // Fork: copy address space.
            if child_stack != 0 {
                debug_warn!("clone: non-zero child_stack without CLONE_VM, ignoring");
            }
            let child = Process::fork(parent, self.frame)?;

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
