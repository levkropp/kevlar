// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! prctl(2) syscall handler.
//!
//! Provenance: Own (Linux prctl(2) man page).
use crate::{
    ctypes::c_int,
    prelude::*,
    process::current_process,
    syscalls::SyscallHandler,
};
use kevlar_platform::address::UserVAddr;

// prctl commands used by systemd.
const PR_SET_PDEATHSIG: c_int = 1;
const PR_SET_NAME: c_int = 15;
const PR_GET_NAME: c_int = 16;
const PR_GET_SECUREBITS: c_int = 27;
const PR_SET_CHILD_SUBREAPER: c_int = 36;
const PR_GET_CHILD_SUBREAPER: c_int = 37;

impl<'a> SyscallHandler<'a> {
    pub fn sys_prctl(
        &mut self,
        option: c_int,
        arg2: usize,
        _arg3: usize,
        _arg4: usize,
        _arg5: usize,
    ) -> Result<isize> {
        match option {
            PR_SET_NAME => {
                let ptr = UserVAddr::new_nonnull(arg2)?;
                // Read up to 16 bytes (including NUL terminator).
                let mut buf = [0u8; 16];
                ptr.read_bytes(&mut buf)?;
                let len = buf.iter().position(|&b| b == 0).unwrap_or(16);
                current_process().set_comm(&buf[..len]);
                Ok(0)
            }
            PR_GET_NAME => {
                let ptr = UserVAddr::new_nonnull(arg2)?;
                let comm = current_process().get_comm();
                let mut buf = [0u8; 16];
                let len = core::cmp::min(comm.len(), 15);
                buf[..len].copy_from_slice(&comm[..len]);
                // buf[len..] is already zeroed.
                ptr.write_bytes(&buf)?;
                Ok(0)
            }
            PR_SET_CHILD_SUBREAPER => {
                current_process().set_child_subreaper(arg2 != 0);
                Ok(0)
            }
            PR_GET_CHILD_SUBREAPER => {
                let ptr = UserVAddr::new_nonnull(arg2)?;
                let val: c_int = if current_process().is_child_subreaper() { 1 } else { 0 };
                ptr.write::<c_int>(&val)?;
                Ok(0)
            }
            PR_SET_PDEATHSIG => {
                // Stub: accept silently.
                Ok(0)
            }
            PR_GET_SECUREBITS => {
                // Return 0 (no secure bits set).
                Ok(0)
            }
            _ => {
                debug_warn!("prctl: unhandled option {}", option);
                Ok(0)
            }
        }
    }
}
