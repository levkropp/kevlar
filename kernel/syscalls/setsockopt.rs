// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! setsockopt(2) syscall handler.
//!
//! Dispatches to per-socket option storage on TcpSocket/UdpSocket.
use crate::{
    ctypes::c_int,
    fs::opened_file::Fd,
    net::{TcpSocket, UdpSocket},
    prelude::*,
    process::current_process,
    syscalls::SyscallHandler,
};
use kevlar_platform::address::UserVAddr;

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
const SO_RCVTIMEO: c_int = 20;
const SO_SNDTIMEO: c_int = 21;

// IPPROTO_TCP options.
const TCP_NODELAY: c_int = 1;

/// Read an int option value from userspace.
fn read_int(optval: usize, optlen: usize) -> Result<c_int> {
    if optlen < 4 {
        return Err(crate::result::Errno::EINVAL.into());
    }
    let ptr = UserVAddr::new_nonnull(optval)?;
    Ok(ptr.read::<c_int>().map_err(|_| crate::result::Error::new(crate::result::Errno::EFAULT))?)
}

/// Read a struct timeval { tv_sec: i64, tv_usec: i64 } and convert to microseconds.
fn read_timeval_us(optval: usize, optlen: usize) -> Result<u64> {
    if optlen < 16 {
        return Err(crate::result::Errno::EINVAL.into());
    }
    let ptr = UserVAddr::new_nonnull(optval)?;
    let tv_sec: i64 = ptr.read().map_err(|_| crate::result::Error::new(crate::result::Errno::EFAULT))?;
    let ptr2 = UserVAddr::new_nonnull(optval + 8)?;
    let tv_usec: i64 = ptr2.read().map_err(|_| crate::result::Error::new(crate::result::Errno::EFAULT))?;
    if tv_sec < 0 || tv_usec < 0 {
        return Ok(0);
    }
    Ok(tv_sec as u64 * 1_000_000 + tv_usec as u64)
}

impl<'a> SyscallHandler<'a> {
    pub fn sys_setsockopt(
        &mut self,
        fd: Fd,
        level: c_int,
        optname: c_int,
        optval: usize,
        optlen: usize,
    ) -> Result<isize> {
        let opened_file = current_process().get_opened_file_by_fd(fd)?;
        let inode = opened_file.inode();
        let file = inode.as_file().ok();

        match level {
            SOL_SOCKET => match optname {
                SO_REUSEADDR | SO_REUSEPORT => {
                    let val = read_int(optval, optlen).unwrap_or(0) != 0;
                    if let Some(f) = &file {
                        if let Some(tcp) = (**f).as_any().downcast_ref::<TcpSocket>() {
                            tcp.set_reuseaddr(val);
                        } else if let Some(udp) = (**f).as_any().downcast_ref::<UdpSocket>() {
                            udp.set_reuseaddr(val);
                        }
                    }
                    Ok(0)
                }
                SO_KEEPALIVE => {
                    let val = read_int(optval, optlen).unwrap_or(0) != 0;
                    if let Some(f) = &file {
                        if let Some(tcp) = (**f).as_any().downcast_ref::<TcpSocket>() {
                            tcp.set_keepalive(val);
                        }
                    }
                    Ok(0)
                }
                SO_RCVTIMEO => {
                    let us = read_timeval_us(optval, optlen).unwrap_or(0);
                    if let Some(f) = &file {
                        if let Some(tcp) = (**f).as_any().downcast_ref::<TcpSocket>() {
                            tcp.set_rcvtimeo(us);
                        } else if let Some(udp) = (**f).as_any().downcast_ref::<UdpSocket>() {
                            udp.set_rcvtimeo(us);
                        }
                    }
                    Ok(0)
                }
                SO_SNDTIMEO => {
                    let us = read_timeval_us(optval, optlen).unwrap_or(0);
                    if let Some(f) = &file {
                        if let Some(tcp) = (**f).as_any().downcast_ref::<TcpSocket>() {
                            tcp.set_sndtimeo(us);
                        }
                    }
                    Ok(0)
                }
                SO_PASSCRED | SO_RCVBUF | SO_SNDBUF => Ok(0), // Accept silently
                _ => {
                    debug_warn!("setsockopt: unhandled SOL_SOCKET option {}", optname);
                    Ok(0)
                }
            },
            IPPROTO_TCP => match optname {
                TCP_NODELAY => {
                    let val = read_int(optval, optlen).unwrap_or(0) != 0;
                    if let Some(f) = &file {
                        if let Some(tcp) = (**f).as_any().downcast_ref::<TcpSocket>() {
                            tcp.set_nodelay(val);
                        }
                    }
                    Ok(0)
                }
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
