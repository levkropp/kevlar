// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::{ctypes::c_int, fs::opened_file::Fd, prelude::*};
use core::mem::size_of;
use kevlar_platform::address::UserVAddr;

use super::SyscallHandler;

const SOL_SOCKET: c_int = 1;
const SO_TYPE: c_int = 3;
const SO_ERROR: c_int = 4;

impl<'a> SyscallHandler<'a> {
    pub fn sys_getsockopt(
        &mut self,
        _fd: Fd,
        level: c_int,
        optname: c_int,
        optval: Option<UserVAddr>,
        optlen: Option<UserVAddr>,
    ) -> Result<isize> {
        match (level, optname) {
            (SOL_SOCKET, SO_ERROR) => {
                // Return 0 (no pending error).
                if let (Some(val), Some(len)) = (optval, optlen) {
                    len.write::<c_int>(&(size_of::<c_int>() as c_int))?;
                    val.write::<c_int>(&0)?;
                }
                Ok(0)
            }
            (SOL_SOCKET, SO_TYPE) => {
                // TODO: Return the actual socket type (SOCK_STREAM, SOCK_DGRAM).
                // For now return SOCK_STREAM (1) as a reasonable default.
                if let (Some(val), Some(len)) = (optval, optlen) {
                    len.write::<c_int>(&(size_of::<c_int>() as c_int))?;
                    val.write::<c_int>(&1)?; // SOCK_STREAM
                }
                Ok(0)
            }
            _ => {
                debug_warn!(
                    "getsockopt: unhandled level={}, optname={}",
                    level,
                    optname
                );
                Ok(0)
            }
        }
    }
}
