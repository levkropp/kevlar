// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::{
    process::current_process,
    result::Result,
    syscalls::SyscallHandler,
};
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    /// sethostname(2) — set hostname in the calling process's UTS namespace.
    pub fn sys_sethostname(&mut self, name: UserVAddr, len: usize) -> Result<isize> {
        let mut buf = [0u8; 64];
        let copy_len = core::cmp::min(len, 64);
        name.read_bytes(&mut buf[..copy_len])?;
        let proc = current_process();
        let ns = proc.namespaces();
        ns.uts.set_hostname(&buf[..copy_len])?;
        // Invalidate the cached utsname so uname(2) sees the new hostname.
        proc.rebuild_cached_utsname();
        Ok(0)
    }

    /// setdomainname(2) — set domainname in the calling process's UTS namespace.
    pub fn sys_setdomainname(&mut self, name: UserVAddr, len: usize) -> Result<isize> {
        let mut buf = [0u8; 64];
        let copy_len = core::cmp::min(len, 64);
        name.read_bytes(&mut buf[..copy_len])?;
        let proc = current_process();
        let ns = proc.namespaces();
        ns.uts.set_domainname(&buf[..copy_len])?;
        proc.rebuild_cached_utsname();
        Ok(0)
    }
}
