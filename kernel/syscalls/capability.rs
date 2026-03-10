// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! capget(2) / capset(2) syscall handlers.
//!
//! Stub: all processes have all capabilities (running as root).
//!
//! Provenance: Own (Linux capabilities(7), capget(2) man pages).
use crate::{prelude::*, syscalls::SyscallHandler};
use kevlar_platform::address::UserVAddr;

/// Linux capability header (version 3 = 0x20080522).
#[repr(C)]
struct CapUserHeader {
    version: u32,
    pid: i32,
}

/// Linux capability data (one set; v3 uses two of these).
#[repr(C)]
struct CapUserData {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

const _LINUX_CAPABILITY_VERSION_3: u32 = 0x20080522;

impl<'a> SyscallHandler<'a> {
    pub fn sys_capget(
        &mut self,
        header_ptr: UserVAddr,
        data_ptr: usize,
    ) -> Result<isize> {
        let header = header_ptr.read::<CapUserHeader>()?;

        if data_ptr == 0 {
            // Caller is just querying the version. Write back v3.
            let resp = CapUserHeader {
                version: _LINUX_CAPABILITY_VERSION_3,
                pid: header.pid,
            };
            header_ptr.write::<CapUserHeader>(&resp)?;
            return Ok(0);
        }

        let data_addr = UserVAddr::new_nonnull(data_ptr)?;

        // Return all capabilities granted (two CapUserData structs for v3).
        let set = CapUserData {
            effective: 0xFFFFFFFF,
            permitted: 0xFFFFFFFF,
            inheritable: 0,
        };
        let stride = core::mem::size_of::<CapUserData>();
        data_addr.write::<CapUserData>(&set)?;
        let data_addr2 = UserVAddr::new_nonnull(data_ptr + stride)?;
        data_addr2.write::<CapUserData>(&set)?;

        Ok(0)
    }

    pub fn sys_capset(
        &mut self,
        _header_ptr: UserVAddr,
        _data_ptr: usize,
    ) -> Result<isize> {
        // Accept silently — we don't enforce capabilities yet.
        Ok(0)
    }
}
