// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::fs::inode::INode;
use crate::net::socket::*;
use crate::result::{Errno, Result};
use crate::{
    ctypes::*,
    fs::opened_file::{OpenOptions, PathComponent},
    services,
};
use crate::{process::current_process, syscalls::SyscallHandler};
use bitflags::bitflags;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct SocketFlags: c_int {
        const SOCK_NONBLOCK = 0o4000;
        const SOCK_CLOEXEC = 0o2000000;
    }
}

impl From<SocketFlags> for OpenOptions {
    fn from(flags: SocketFlags) -> OpenOptions {
        OpenOptions {
            nonblock: flags.contains(SocketFlags::SOCK_NONBLOCK),
            close_on_exec: flags.contains(SocketFlags::SOCK_CLOEXEC),
            append: false,
        }
    }
}

const SOCKET_TYPE_MASK: c_int = 0xff;

impl<'a> SyscallHandler<'a> {
    pub fn sys_socket(&mut self, domain: i32, type_: i32, protocol: i32) -> Result<isize> {
        let socket_type = type_ & SOCKET_TYPE_MASK;
        let flags = bitflags_from_user!(SocketFlags, type_ & !SOCKET_TYPE_MASK)?;

        let net = services::network_stack();
        let socket = match (domain, socket_type, protocol) {
            (AF_UNIX, SOCK_STREAM, 0) => net.create_unix_socket()?,
            (AF_INET, SOCK_DGRAM, 0) | (AF_INET, SOCK_DGRAM, IPPROTO_UDP) => {
                net.create_udp_socket()?
            }
            (AF_INET, SOCK_STREAM, 0) | (AF_INET, SOCK_STREAM, IPPROTO_TCP) => {
                net.create_tcp_socket()?
            }
            (_, _, _) => {
                debug_warn!(
                    "unsupported socket type: domain={}, type={}, protocol={}",
                    domain,
                    type_,
                    protocol
                );
                return Err(Errno::ENOSYS.into());
            }
        };

        let fd = current_process().opened_files().lock().open(
            PathComponent::new_anonymous(INode::FileLike(socket)),
            flags.into(),
        )?;

        Ok(fd.as_usize() as isize)
    }
}
