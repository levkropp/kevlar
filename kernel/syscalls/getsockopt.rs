// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::{
    ctypes::c_int,
    fs::opened_file::Fd,
    net::{TcpSocket, UdpSocket},
    prelude::*,
    process::current_process,
};
use core::mem::size_of;
use kevlar_platform::address::UserVAddr;

use super::SyscallHandler;

const SOL_SOCKET: c_int = 1;
const IPPROTO_TCP: c_int = 6;
const SO_REUSEADDR: c_int = 2;
const SO_TYPE: c_int = 3;
const SO_ERROR: c_int = 4;
const SO_SNDBUF: c_int = 7;
const SO_RCVBUF: c_int = 8;
const SO_KEEPALIVE: c_int = 9;
const SO_PEERCRED: c_int = 17;
const SO_RCVTIMEO: c_int = 20;
const SO_SNDTIMEO: c_int = 21;
const SO_PASSCRED: c_int = 16;
const TCP_NODELAY: c_int = 1;

fn write_int_opt(optval: Option<UserVAddr>, optlen: Option<UserVAddr>, value: c_int) -> Result<()> {
    if let (Some(val), Some(len)) = (optval, optlen) {
        len.write::<c_int>(&(size_of::<c_int>() as c_int))?;
        val.write::<c_int>(&value)?;
    }
    Ok(())
}

fn write_timeval_opt(optval: Option<UserVAddr>, optlen: Option<UserVAddr>, us: u64) -> Result<()> {
    if let (Some(val), Some(len)) = (optval, optlen) {
        len.write::<c_int>(&(16 as c_int))?; // sizeof(struct timeval) = 16
        let tv_sec = (us / 1_000_000) as i64;
        let tv_usec = (us % 1_000_000) as i64;
        val.write::<i64>(&tv_sec)?;
        let val2 = UserVAddr::new_nonnull(val.as_isize() as usize + 8)?;
        val2.write::<i64>(&tv_usec)?;
    }
    Ok(())
}

/// Check whether the fd refers to a socket that has a pending error.
fn socket_error(fd: Fd) -> c_int {
    use crate::fs::inode::PollStatus;
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
                let proc = current_process();
                let table = proc.opened_files().lock();
                let stype = if let Ok(of) = table.get(fd) {
                    if let Ok(file) = of.inode().as_file() {
                        let t = file.socket_type();
                        if t != 0 { t } else { 1 }
                    } else { 1 }
                } else { 1 };
                write_int_opt(optval, optlen, stype)?;
                Ok(0)
            }
            (SOL_SOCKET, SO_RCVBUF) => {
                write_int_opt(optval, optlen, 212992)?;
                Ok(0)
            }
            (SOL_SOCKET, SO_SNDBUF) => {
                write_int_opt(optval, optlen, 16384)?;
                Ok(0)
            }
            (SOL_SOCKET, SO_REUSEADDR) => {
                let val = self.get_socket_bool(fd, |tcp| tcp.reuseaddr(), |udp| udp.reuseaddr());
                write_int_opt(optval, optlen, val as c_int)?;
                Ok(0)
            }
            (SOL_SOCKET, SO_KEEPALIVE) => {
                let val = self.get_socket_bool(fd, |tcp| tcp.keepalive(), |_udp| false);
                write_int_opt(optval, optlen, val as c_int)?;
                Ok(0)
            }
            (SOL_SOCKET, SO_RCVTIMEO) => {
                let us = self.get_socket_u64(fd, |tcp| tcp.rcvtimeo_us(), |udp| udp.rcvtimeo_us());
                write_timeval_opt(optval, optlen, us)?;
                Ok(0)
            }
            (SOL_SOCKET, SO_SNDTIMEO) => {
                let us = self.get_socket_u64(fd, |tcp| tcp.sndtimeo_us(), |_udp| 0);
                write_timeval_opt(optval, optlen, us)?;
                Ok(0)
            }
            (SOL_SOCKET, SO_PEERCRED) => {
                // struct ucred { pid_t pid; uid_t uid; gid_t gid; } = 12 bytes
                // For local Unix sockets, return the current process's credentials.
                // Remote TCP sockets: return current process (simplification).
                let proc = current_process();
                let pid = proc.pid().as_i32();
                let ucred: [u8; 12] = {
                    let mut buf = [0u8; 12];
                    buf[0..4].copy_from_slice(&pid.to_le_bytes());
                    buf[4..8].copy_from_slice(&0u32.to_le_bytes()); // uid=0 (root)
                    buf[8..12].copy_from_slice(&0u32.to_le_bytes()); // gid=0 (root)
                    buf
                };
                if let (Some(val), Some(len)) = (optval, optlen) {
                    len.write::<c_int>(&12)?;
                    val.write_bytes(&ucred)?;
                }
                Ok(0)
            }
            (SOL_SOCKET, SO_PASSCRED) => {
                // Report that SCM_CREDENTIALS passing is enabled
                write_int_opt(optval, optlen, 1)?;
                Ok(0)
            }
            (IPPROTO_TCP, TCP_NODELAY) => {
                let val = self.get_socket_bool(fd, |tcp| tcp.nodelay(), |_udp| false);
                write_int_opt(optval, optlen, val as c_int)?;
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

    /// Helper: read a bool from TcpSocket or UdpSocket via downcast.
    fn get_socket_bool(
        &self,
        fd: Fd,
        tcp_fn: impl Fn(&TcpSocket) -> bool,
        udp_fn: impl Fn(&UdpSocket) -> bool,
    ) -> bool {
        let Ok(opened_file) = current_process().get_opened_file_by_fd(fd) else { return false };
        let inode = opened_file.inode();
        if let Ok(f) = inode.as_file() {
            if let Some(tcp) = (**f).as_any().downcast_ref::<TcpSocket>() {
                return tcp_fn(tcp);
            }
            if let Some(udp) = (**f).as_any().downcast_ref::<UdpSocket>() {
                return udp_fn(udp);
            }
        }
        false
    }

    /// Helper: read a u64 from TcpSocket or UdpSocket via downcast.
    fn get_socket_u64(
        &self,
        fd: Fd,
        tcp_fn: impl Fn(&TcpSocket) -> u64,
        udp_fn: impl Fn(&UdpSocket) -> u64,
    ) -> u64 {
        let Ok(opened_file) = current_process().get_opened_file_by_fd(fd) else { return 0 };
        let inode = opened_file.inode();
        if let Ok(f) = inode.as_file() {
            if let Some(tcp) = (**f).as_any().downcast_ref::<TcpSocket>() {
                return tcp_fn(tcp);
            }
            if let Some(udp) = (**f).as_any().downcast_ref::<UdpSocket>() {
                return udp_fn(udp);
            }
        }
        0
    }
}
