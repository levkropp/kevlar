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
/// Per POSIX, reading SO_ERROR clears the error (we don't track per-socket
/// errors yet, so we just check whether the socket is in an error state).
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
            // The socket is in an error state.  For TCP, this could be
            // ECONNREFUSED (connect failed) or ECONNRESET (peer reset).
            // Check POLLHUP to distinguish: POLLHUP without prior data
            // exchange typically means ECONNREFUSED.
            if status.contains(PollStatus::POLLHUP) {
                return 104; // ECONNRESET
            }
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
                use crate::process::current_process;
                let proc = current_process();
                let table = proc.opened_files().lock();
                let stype = if let Ok(of) = table.get(fd) {
                    if let Ok(file) = of.inode().as_file() {
                        let t = file.socket_type();
                        if t != 0 { t } else { 1 } // default SOCK_STREAM
                    } else { 1 }
                } else { 1 };
                write_int_opt(optval, optlen, stype)?;
                Ok(0)
            }
            (SOL_SOCKET, SO_RCVBUF) => {
                // Linux doubles the value set via setsockopt when returning
                // it via getsockopt (to account for kernel bookkeeping).
                // The default rmem_default is 212992.
                write_int_opt(optval, optlen, 212992)?;
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
