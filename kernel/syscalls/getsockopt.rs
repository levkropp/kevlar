// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::{ctypes::c_int, fs::opened_file::Fd, prelude::*};
use core::mem::size_of;
use kevlar_platform::address::UserVAddr;

use super::SyscallHandler;

const SOL_SOCKET: c_int = 1;
const SO_TYPE: c_int = 3;
const SO_ERROR: c_int = 4;
const SO_KEEPALIVE: c_int = 9;
const SO_RCVBUF: c_int = 8;
const SO_SNDBUF: c_int = 7;

fn write_int_opt(optval: Option<UserVAddr>, optlen: Option<UserVAddr>, value: c_int) -> Result<()> {
    if let (Some(val), Some(len)) = (optval, optlen) {
        len.write::<c_int>(&(size_of::<c_int>() as c_int))?;
        val.write::<c_int>(&value)?;
    }
    Ok(())
}

/// Check whether the fd refers to a socket that has a pending error.
/// Returns the errno value (e.g. ECONNREFUSED=111) or 0 if no error.
fn socket_error(fd: Fd) -> c_int {
    use crate::fs::inode::PollStatus;
    use crate::process::current_process;
    let proc = current_process();
    let table = proc.opened_files().lock();
    let Ok(opened_file) = table.get(fd) else {
        return 0;
    };
    let inode = opened_file.inode();
    let file = match inode {
        kevlar_vfs::inode::INode::FileLike(f) => f,
        _ => return 0,
    };
    if let Ok(status) = file.poll() {
        if status.contains(PollStatus::POLLERR) {
            return 111; // ECONNREFUSED
        }
    }
    0
}

impl<'a> SyscallHandler<'a> {
    pub fn sys_getsockopt(
        &mut self,
        fd: Fd,
        level: c_int,
        optname: c_int,
        optval: Option<UserVAddr>,
        optlen: Option<UserVAddr>,
    ) -> Result<isize> {
        match (level, optname) {
            (SOL_SOCKET, SO_ERROR) => {
                let err = socket_error(fd);
                write_int_opt(optval, optlen, err)?;
                Ok(0)
            }
            (SOL_SOCKET, SO_TYPE) => {
                // TODO: Return the actual socket type (SOCK_STREAM, SOCK_DGRAM).
                write_int_opt(optval, optlen, 1)?; // SOCK_STREAM
                Ok(0)
            }
            (SOL_SOCKET, SO_RCVBUF) => {
                write_int_opt(optval, optlen, 87380)?;
                Ok(0)
            }
            (SOL_SOCKET, SO_SNDBUF) => {
                write_int_opt(optval, optlen, 16384)?;
                Ok(0)
            }
            (SOL_SOCKET, SO_KEEPALIVE) => {
                write_int_opt(optval, optlen, 0)?;
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
