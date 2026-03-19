// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX setresuid(2), setresgid(2), getresuid(2), getresgid(2) man pages).
//
// Real/effective/saved UID/GID management. Required by musl, PAM, su, login.
use crate::process::current_process;
use crate::prelude::*;
use crate::syscalls::SyscallHandler;
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    /// setresuid(ruid, euid, suid) — set real, effective, and saved UIDs.
    /// A value of -1 (0xFFFFFFFF as u32) means "don't change".
    pub fn sys_setresuid(&mut self, ruid: u32, euid: u32, suid: u32) -> Result<isize> {
        let proc = current_process();
        if ruid != 0xFFFFFFFF {
            proc.set_uid(ruid);
        }
        if euid != 0xFFFFFFFF {
            proc.set_euid(euid);
        }
        if suid != 0xFFFFFFFF {
            proc.set_suid(suid);
        }
        Ok(0)
    }

    /// setresgid(rgid, egid, sgid) — set real, effective, and saved GIDs.
    pub fn sys_setresgid(&mut self, rgid: u32, egid: u32, sgid: u32) -> Result<isize> {
        let proc = current_process();
        if rgid != 0xFFFFFFFF {
            proc.set_gid(rgid);
        }
        if egid != 0xFFFFFFFF {
            proc.set_egid(egid);
        }
        if sgid != 0xFFFFFFFF {
            proc.set_sgid(sgid);
        }
        Ok(0)
    }

    /// getresuid(*ruid, *euid, *suid) — get real, effective, and saved UIDs.
    pub fn sys_getresuid(&mut self, ruid_ptr: UserVAddr, euid_ptr: UserVAddr, suid_ptr: UserVAddr) -> Result<isize> {
        let proc = current_process();
        ruid_ptr.write::<u32>(&proc.uid())?;
        euid_ptr.write::<u32>(&proc.euid())?;
        suid_ptr.write::<u32>(&proc.suid())?;
        Ok(0)
    }

    /// getresgid(*rgid, *egid, *sgid) — get real, effective, and saved GIDs.
    pub fn sys_getresgid(&mut self, rgid_ptr: UserVAddr, egid_ptr: UserVAddr, sgid_ptr: UserVAddr) -> Result<isize> {
        let proc = current_process();
        rgid_ptr.write::<u32>(&proc.gid())?;
        egid_ptr.write::<u32>(&proc.egid())?;
        sgid_ptr.write::<u32>(&proc.sgid())?;
        Ok(0)
    }
}
