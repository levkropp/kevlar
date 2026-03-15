// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::fs::inode::{FileLike, INode};

use crate::net::service::NetworkStackService;
use crate::net::socket::*;
use crate::net::UnixStream;
use crate::result::{Errno, Result};
use crate::{
    ctypes::*,
    fs::opened_file::{OpenOptions, PathComponent},
    prelude::*,
    services,
};
use crate::{process::current_process, syscalls::SyscallHandler};
use bitflags::bitflags;
use kevlar_platform::address::UserVAddr;

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
            access_mode: 2,
        }
    }
}

const SOCKET_TYPE_MASK: c_int = 0xff;

impl<'a> SyscallHandler<'a> {
    pub fn sys_socket(&mut self, domain: i32, type_: i32, protocol: i32) -> Result<isize> {
        let socket_type = type_ & SOCKET_TYPE_MASK;
        let flags = bitflags_from_user!(SocketFlags, type_ & !SOCKET_TYPE_MASK)?;

        let net = services::network_stack();
        let socket = services::call_service(|| {
            match (domain, socket_type, protocol) {
                (AF_UNIX, SOCK_STREAM, 0) => net.create_unix_socket(),
                (AF_INET, SOCK_DGRAM, 0) | (AF_INET, SOCK_DGRAM, IPPROTO_UDP) => {
                    net.create_udp_socket()
                }
                (AF_INET, SOCK_STREAM, 0) | (AF_INET, SOCK_STREAM, IPPROTO_TCP) => {
                    net.create_tcp_socket()
                }
                (_, _, _) => {
                    debug_warn!(
                        "unsupported socket type: domain={}, type={}, protocol={}",
                        domain,
                        type_,
                        protocol
                    );
                    Err(Errno::ENOSYS.into())
                }
            }
        })?;

        let fd = current_process().opened_files().lock().open(
            PathComponent::new_anonymous(INode::FileLike(socket)),
            flags.into(),
        )?;

        Ok(fd.as_usize() as isize)
    }

    pub fn sys_socketpair(
        &mut self,
        domain: i32,
        type_: i32,
        protocol: i32,
        sv_ptr: UserVAddr,
    ) -> Result<isize> {
        let socket_type = type_ & SOCKET_TYPE_MASK;
        let flags = bitflags_from_user!(SocketFlags, type_ & !SOCKET_TYPE_MASK)?;
        let options: OpenOptions = flags.into();

        if domain != AF_UNIX || socket_type != SOCK_STREAM || protocol != 0 {
            return Err(Errno::ENOSYS.into());
        }

        let (a, b) = UnixStream::new_pair();

        let mut table = current_process().opened_files().lock();
        let fd0 = table.open(
            PathComponent::new_anonymous(INode::FileLike(a as Arc<dyn FileLike>)),
            options,
        )?;
        let fd1 = table.open(
            PathComponent::new_anonymous(INode::FileLike(b as Arc<dyn FileLike>)),
            options,
        )?;

        // Write [fd0, fd1] to userspace.
        sv_ptr.write::<i32>(&fd0.as_int())?;
        sv_ptr.add(4).write::<i32>(&fd1.as_int())?;

        Ok(0)
    }
}
