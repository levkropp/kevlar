// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Socket types. Re-exports raw types from kevlar_vfs plus smoltcp conversions
//! and kernel-internal read/write helpers.
pub use kevlar_vfs::socket_types::*;

use crate::result::*;
use core::mem::size_of;
use kevlar_platform::address::UserVAddr;
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address};

/// Convert a `SockAddr` to a smoltcp `IpEndpoint`.
pub fn sockaddr_to_endpoint(sockaddr: SockAddr) -> Result<IpEndpoint> {
    match sockaddr {
        SockAddr::In(SockAddrIn { port, addr, .. }) => Ok(IpEndpoint {
            port: u16::from_be_bytes(port),
            addr: IpAddress::Ipv4(Ipv4Address::from(addr)),
        }),
        _ => Err(Errno::EINVAL.into()),
    }
}

/// Convert a smoltcp `IpEndpoint` to a `SockAddr`.
pub fn endpoint_to_sockaddr(endpoint: IpEndpoint) -> SockAddr {
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
            // TODO: Should we check `len` for sockaddr_un as well?
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
        _ => return Err(Errno::EINVAL.into()),
    }

    Ok(())
}
