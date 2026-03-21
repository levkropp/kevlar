// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Socket type definitions used in `FileLike` trait method signatures.
//!
//! These are raw ABI types. Protocol-specific conversions (e.g., to smoltcp
//! `IpEndpoint`) live in the network stack service crate.
use bitflags::bitflags;

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
pub const AF_NETLINK: i32 = 16;
pub const AF_PACKET: i32 = 17;
pub const SOCK_STREAM: i32 = 1;
pub const SOCK_DGRAM: i32 = 2;
pub const SOCK_RAW: i32 = 3;
pub const IPPROTO_ICMP: i32 = 1;
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
    Nl(SockAddrNl),
}

/// `struct sockaddr_in`
#[derive(Debug, Copy, Clone)]
#[repr(C, packed)]
pub struct SockAddrIn {
    /// `AF_INET`
    pub family: sa_family_t,
    /// The port number in the network byte order.
    pub port: [u8; 2],
    /// The IPv4 address in the network byte order.
    pub addr: [u8; 4],
    /// Unused padding area.
    pub zero: [u8; 8],
}

/// `struct sockaddr_un`
#[derive(Debug, Copy, Clone)]
#[repr(C, packed)]
pub struct SockAddrUn {
    /// `AF_UNIX`
    pub family: sa_family_t,
    /// The unix domain socket file path.
    pub path: [u8; 108],
}

/// `struct sockaddr_nl`
#[derive(Debug, Copy, Clone)]
#[repr(C, packed)]
pub struct SockAddrNl {
    pub family: sa_family_t,
    pub pad: u16,
    pub pid: u32,
    pub groups: u32,
}
