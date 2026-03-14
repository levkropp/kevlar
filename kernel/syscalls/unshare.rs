// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::{
    namespace::{CLONE_NEWNET, CLONE_NEWNS, CLONE_NEWPID, CLONE_NEWUTS},
    process::current_process,
    result::{Errno, Result},
    syscalls::SyscallHandler,
};

impl<'a> SyscallHandler<'a> {
    /// unshare(2) — create new namespaces for the calling process.
    pub fn sys_unshare(&mut self, flags: usize) -> Result<isize> {
        if flags & CLONE_NEWNET != 0 {
            return Err(Errno::EINVAL.into());
        }

        let ns_flags = flags & (CLONE_NEWUTS | CLONE_NEWPID | CLONE_NEWNS);
        if ns_flags == 0 {
            return Ok(0); // nothing to do
        }

        let proc = current_process();
        let old_ns = proc.namespaces();
        let new_ns = old_ns.clone_with_flags(ns_flags)?;
        proc.set_namespaces(new_ns);
        Ok(0)
    }
}
