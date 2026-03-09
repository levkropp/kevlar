// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::result::*;
use bitflags::bitflags;
use core::convert::TryFrom;
use core::mem::size_of;
use kevlar_platform::address::UserVAddr;
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address};

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct RecvFromFlags: i32 {
        // TODO:
        const _NOT_IMPLEMENTED = 0x1;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct SendToFlags: i32 {
        // TODO: remaining flags
        const MSG_NOSIGNAL = 0x4000;
    }
}

pub const AF_UNIX: i32 = 1;
pub const AF_INET: i32 = 2;
pub const SOCK_STREAM: i32 = 1;
pub const SOCK_DGRAM: i32 = 2;
pub const IPPROTO_TCP: i32 = 6;
pub const IPPROTO_UDP: i32 = 17;

#[allow(non_camel_case_types)]
pub type sa_family_t = u16;
#[allow(non_camel_case_types)]
pub type socklen_t = u32;

/// The `how` argument in `shutdown(2)`.
#[repr(i32)]
pub enum ShutdownHow {
    /// `SHUT_RD`.
    Rd = 0,
    /// `SHUT_WR`.
    Wr = 1,
    /// `SHUT_RDWR`.
    RdWr = 2,
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum SockAddr {
    In(SockAddrIn),
    Un(SockAddrUn),
}

/// `struct sockaddr_in`
#[derive(Debug, Copy, Clone)]
#[repr(C, packed)]
pub struct SockAddrIn {
    /// `AF_INET`
    family: sa_family_t,
    /// The port number in the network byte order.
    port: [u8; 2],
    /// The IPv4 address in the network byte order.
    addr: [u8; 4],
    /// Unused padding area.
    zero: [u8; 8],
}

/// `struct sockaddr_un`
#[derive(Debug, Copy, Clone)]
#[repr(C, packed)]
pub struct SockAddrUn {
    /// `AF_UNIX`
    family: sa_family_t,
    /// The unix domain socket file path.
    path: [u8; 108],
}

impl TryFrom<SockAddr> for IpEndpoint {
    type Error = Error;
    fn try_from(sockaddr: SockAddr) -> Result<IpEndpoint> {
        match sockaddr {
            SockAddr::In(SockAddrIn { port, addr, .. }) => Ok(IpEndpoint {
                port: u16::from_be_bytes(port),
                addr: IpAddress::Ipv4(Ipv4Address::from(addr)),
            }),
            _ => Err(Errno::EINVAL.into()),
        }
    }
}

impl From<IpEndpoint> for SockAddr {
    fn from(endpoint: IpEndpoint) -> SockAddr {
        SockAddr::In(SockAddrIn {
            family: AF_INET as u16,
            port: endpoint.port.to_be_bytes(),
            addr: match endpoint.addr {
                IpAddress::Ipv4(addr) => addr.octets(),
                #[allow(unreachable_patterns)]
                _ => Ipv4Address::UNSPECIFIED.octets(),
            },
            zero: [0; 8],
        })
    }
}

pub fn read_sockaddr(uaddr: UserVAddr, len: usize) -> Result<SockAddr> {
    let sa_family = uaddr.read::<sa_family_t>()?;
    let sockaddr = match sa_family as i32 {
        AF_INET => {
            if len < size_of::<SockAddrIn>() {
                return Err(Errno::EINVAL.into());
            }

            SockAddr::In(uaddr.read::<SockAddrIn>()?)
        }
        AF_UNIX => {
            // TODO: SHould we check `len` for sockaddr_un as well?
            SockAddr::Un(uaddr.read::<SockAddrUn>()?)
        }
        _ => {
            return Err(Errno::EINVAL.into());
        }
    };

    Ok(sockaddr)
}

pub fn write_sockaddr(
    sockaddr: &SockAddr,
    dst: Option<UserVAddr>,
    socklen: Option<UserVAddr>,
) -> Result<()> {
    match sockaddr {
        SockAddr::In(sockaddr_in) => {
            if let Some(dst) = dst {
                dst.write::<SockAddrIn>(sockaddr_in)?;
            }

            if let Some(socklen) = socklen {
                socklen.write::<socklen_t>(&(size_of::<SockAddrIn>() as u32))?;
            }
        }
        SockAddr::Un(sockaddr_un) => {
            if let Some(dst) = dst {
                dst.write::<SockAddrUn>(sockaddr_un)?;
            }

            if let Some(socklen) = socklen {
                socklen.write::<socklen_t>(&(size_of::<SockAddrUn>() as u32))?;
            }
        }
    }

    Ok(())
}
