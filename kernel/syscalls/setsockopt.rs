// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! setsockopt(2) syscall handler.
//!
//! Stubs for the minimum options needed by systemd + D-Bus.
//!
//! Provenance: Own (Linux setsockopt(2) man page).
use crate::{
    ctypes::c_int,
    fs::opened_file::Fd,
    prelude::*,
    syscalls::SyscallHandler,
};

// Option levels.
const SOL_SOCKET: c_int = 1;
const IPPROTO_TCP: c_int = 6;

// SOL_SOCKET options.
const SO_REUSEADDR: c_int = 2;
const SO_KEEPALIVE: c_int = 9;
const SO_PASSCRED: c_int = 16;
const SO_RCVBUF: c_int = 8;
const SO_SNDBUF: c_int = 7;
const SO_REUSEPORT: c_int = 15;

// IPPROTO_TCP options.
const TCP_NODELAY: c_int = 1;

impl<'a> SyscallHandler<'a> {
    pub fn sys_setsockopt(
        &mut self,
        _fd: Fd,
        level: c_int,
        optname: c_int,
        _optval: usize,
        _optlen: usize,
    ) -> Result<isize> {
        match level {
            SOL_SOCKET => match optname {
                SO_REUSEADDR | SO_KEEPALIVE | SO_PASSCRED | SO_RCVBUF | SO_SNDBUF
                | SO_REUSEPORT => {
                    // Accept silently — these are stubs.
                    Ok(0)
                }
                _ => {
                    debug_warn!("setsockopt: unhandled SOL_SOCKET option {}", optname);
                    Ok(0)
                }
            },
            IPPROTO_TCP => match optname {
                TCP_NODELAY => Ok(0), // stub
                _ => {
                    debug_warn!("setsockopt: unhandled TCP option {}", optname);
                    Ok(0)
                }
            },
            _ => {
                debug_warn!("setsockopt: unhandled level {}", level);
                Ok(0)
            }
        }
    }
}
