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
        AF_NETLINK => {
            if len < size_of::<SockAddrNl>() {
                return Err(Errno::EINVAL.into());
            }
            SockAddr::Nl(uaddr.read::<SockAddrNl>()?)
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
    // Read the caller's max buffer size from socklen to avoid overflowing
    // the user buffer. On Linux, sockaddr writes are truncated to the
    // provided buffer size. We always write back the FULL address length.
    let max_len = if let Some(sl) = socklen {
        sl.read::<socklen_t>()? as usize
    } else {
        usize::MAX
    };

    match sockaddr {
        SockAddr::In(sockaddr_in) => {
            let full_len = size_of::<SockAddrIn>();
            if let Some(dst) = dst {
                if max_len >= full_len {
                    dst.write::<SockAddrIn>(sockaddr_in)?;
                } else if max_len > 0 {
                    let fam = { sockaddr_in.family };
                    dst.write::<u16>(&fam)?;
                }
            }
            if let Some(socklen) = socklen {
                socklen.write::<socklen_t>(&(full_len as u32))?;
            }
        }
        SockAddr::Un(sockaddr_un) => {
            let full_len = size_of::<SockAddrUn>();
            if let Some(dst) = dst {
                if max_len >= full_len {
                    dst.write::<SockAddrUn>(sockaddr_un)?;
                } else if max_len >= 2 {
                    // Truncated: write family + as much path as fits
                    let fam = { sockaddr_un.family };
                    dst.write::<u16>(&fam)?;
                    let path = { sockaddr_un.path };
                    let copy_len = core::cmp::min(path.len(), max_len - 2);
                    dst.add(2).write_bytes(&path[..copy_len])?;
                }
            }
            if let Some(socklen) = socklen {
                socklen.write::<socklen_t>(&(full_len as u32))?;
            }
        }
        SockAddr::Nl(sockaddr_nl) => {
            let full_len = size_of::<SockAddrNl>();
            if let Some(dst) = dst {
                if max_len >= full_len {
                    dst.write::<SockAddrNl>(sockaddr_nl)?;
                }
            }
            if let Some(socklen) = socklen {
                socklen.write::<socklen_t>(&(full_len as u32))?;
            }
        }
        #[allow(unreachable_patterns)]
        _ => return Err(Errno::EINVAL.into()),
    }

    Ok(())
}
