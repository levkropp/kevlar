// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Network interface ioctls (SIOCGIF*, SIOCSIF*, etc.)
//!
//! BusyBox `ifconfig` uses these to query/set interface configuration.
//! We read from the global smoltcp interface and accept SET ioctls silently
//! since the kernel already manages the network state.
use crate::ctypes::*;
use crate::net::{use_ethernet_driver, INTERFACE};
use crate::result::{Errno, Result};
use crate::syscalls::SyscallHandler;
use kevlar_platform::address::UserVAddr;
use smoltcp::wire::IpCidr;

// ioctl command numbers
const SIOCADDRT: usize = 0x890b;
const SIOCDELRT: usize = 0x890c;
const SIOCGIFCONF: usize = 0x8912;
const SIOCGIFFLAGS: usize = 0x8913;
const SIOCSIFFLAGS: usize = 0x8914;
const SIOCGIFADDR: usize = 0x8915;
const SIOCSIFADDR: usize = 0x8916;
const SIOCGIFNETMASK: usize = 0x891b;
const SIOCSIFNETMASK: usize = 0x891c;
const SIOCGIFMTU: usize = 0x8921;
const SIOCGIFHWADDR: usize = 0x8927;
const SIOCGIFINDEX: usize = 0x8933;

// Interface flags
const IFF_UP: c_short = 0x1;
const IFF_BROADCAST: c_short = 0x2;
const IFF_LOOPBACK: c_short = 0x8;
const IFF_RUNNING: c_short = 0x40;

// struct ifreq layout: 16-byte name + 24-byte union = 40 bytes
const IFNAMSIZ: usize = 16;

/// Reads the interface name from the ifreq struct at `arg`.
/// Returns the name as a byte slice (without trailing NUL).
fn read_ifr_name(arg: UserVAddr) -> Result<[u8; IFNAMSIZ]> {
    let mut name = [0u8; IFNAMSIZ];
    arg.read_bytes(&mut name)?;
    Ok(name)
}

/// Returns the string portion of an ifr_name (up to the first NUL).
fn ifr_name_str(name: &[u8; IFNAMSIZ]) -> &[u8] {
    let len = name.iter().position(|&b| b == 0).unwrap_or(IFNAMSIZ);
    &name[..len]
}

/// Checks if the interface name matches "eth0" or "lo".
/// Returns true for eth0, false for lo, or ENODEV for unknown.
fn classify_iface(name: &[u8; IFNAMSIZ]) -> Result<bool> {
    let s = ifr_name_str(name);
    if s == b"eth0" {
        Ok(true)
    } else if s == b"lo" {
        Ok(false)
    } else {
        Err(Errno::ENODEV.into())
    }
}

/// Converts a CIDR prefix length to a network-byte-order IPv4 netmask.
fn prefix_len_to_netmask(prefix: u8) -> [u8; 4] {
    if prefix == 0 {
        return [0, 0, 0, 0];
    }
    let mask: u32 = !0u32 << (32 - prefix as u32);
    mask.to_be_bytes()
}

/// Writes a sockaddr_in structure into the ifreq union area at `arg + 16`.
fn write_sockaddr_in(arg: UserVAddr, addr: [u8; 4]) -> Result<()> {
    let sa_offset = arg.add(IFNAMSIZ);
    // struct sockaddr_in: family(2) + port(2) + addr(4) + zero(8) = 16 bytes
    let mut sa = [0u8; 16];
    sa[0] = 2; // AF_INET (little-endian u16)
    sa[1] = 0;
    // port = 0
    sa[4] = addr[0];
    sa[5] = addr[1];
    sa[6] = addr[2];
    sa[7] = addr[3];
    sa_offset.write_bytes(&sa)?;
    Ok(())
}

/// Gets the current IPv4 address and prefix length from the smoltcp interface.
fn get_ipv4_addr() -> ([u8; 4], u8) {
    let iface = INTERFACE.lock();
    for cidr in iface.ip_addrs() {
        let IpCidr::Ipv4(v4cidr) = cidr;
        return (v4cidr.address().octets(), v4cidr.prefix_len());
    }
    ([0, 0, 0, 0], 0)
}

impl<'a> SyscallHandler<'a> {
    pub fn sys_net_ioctl(&mut self, cmd: usize, arg: usize) -> Result<isize> {
        let arg = UserVAddr::new_nonnull(arg)?;

        match cmd {
            SIOCGIFFLAGS => {
                let name = read_ifr_name(arg)?;
                let is_eth0 = classify_iface(&name)?;
                let flags: c_short = if is_eth0 {
                    IFF_UP | IFF_RUNNING | IFF_BROADCAST
                } else {
                    IFF_UP | IFF_RUNNING | IFF_LOOPBACK
                };
                // Write flags as i16 at offset 16 in ifreq
                arg.add(IFNAMSIZ).write::<c_short>(&flags)?;
                Ok(0)
            }

            SIOCSIFFLAGS | SIOCSIFADDR | SIOCSIFNETMASK | SIOCADDRT | SIOCDELRT => {
                // Accept silently — kernel manages network state.
                Ok(0)
            }

            SIOCGIFADDR => {
                let name = read_ifr_name(arg)?;
                let is_eth0 = classify_iface(&name)?;
                if is_eth0 {
                    let (addr, _) = get_ipv4_addr();
                    write_sockaddr_in(arg, addr)?;
                } else {
                    // lo: 127.0.0.1
                    write_sockaddr_in(arg, [127, 0, 0, 1])?;
                }
                Ok(0)
            }

            SIOCGIFNETMASK => {
                let name = read_ifr_name(arg)?;
                let is_eth0 = classify_iface(&name)?;
                if is_eth0 {
                    let (_, prefix) = get_ipv4_addr();
                    let mask = prefix_len_to_netmask(prefix);
                    write_sockaddr_in(arg, mask)?;
                } else {
                    // lo: 255.0.0.0
                    write_sockaddr_in(arg, [255, 0, 0, 0])?;
                }
                Ok(0)
            }

            SIOCGIFHWADDR => {
                let name = read_ifr_name(arg)?;
                let is_eth0 = classify_iface(&name)?;
                // struct sockaddr at offset 16: family(2) + data(14)
                let sa_offset = arg.add(IFNAMSIZ);
                if is_eth0 {
                    let mac = use_ethernet_driver(|d| d.mac_addr());
                    let mut sa = [0u8; 16];
                    sa[0] = 1; // ARPHRD_ETHER = 1 (little-endian u16)
                    sa[2..8].copy_from_slice(&mac.as_array());
                    sa_offset.write_bytes(&sa)?;
                } else {
                    // lo: ARPHRD_LOOPBACK = 772, all-zero MAC
                    let mut sa = [0u8; 16];
                    let arphrd: u16 = 772;
                    sa[0..2].copy_from_slice(&arphrd.to_le_bytes());
                    sa_offset.write_bytes(&sa)?;
                }
                Ok(0)
            }

            SIOCGIFINDEX => {
                let name = read_ifr_name(arg)?;
                let is_eth0 = classify_iface(&name)?;
                let index: c_int = if is_eth0 { 1 } else { 2 };
                arg.add(IFNAMSIZ).write::<c_int>(&index)?;
                Ok(0)
            }

            SIOCGIFMTU => {
                let name = read_ifr_name(arg)?;
                let is_eth0 = classify_iface(&name)?;
                let mtu: c_int = if is_eth0 { 1500 } else { 65536 };
                arg.add(IFNAMSIZ).write::<c_int>(&mtu)?;
                Ok(0)
            }

            SIOCGIFCONF => {
                // struct ifconf: { int ifc_len; union { char* ifc_buf; struct ifreq* ifc_req; } }
                // On x86_64: ifc_len at offset 0 (4 bytes), ifc_req at offset 8 (8 bytes, pointer)
                let ifc_len: c_int = arg.read::<c_int>()?;
                let ifc_buf_ptr: usize = arg.add(8).read::<usize>()?;

                if ifc_buf_ptr == 0 || ifc_len == 0 {
                    // Query mode: return required buffer size.
                    // 2 interfaces * 40 bytes each = 80
                    arg.write::<c_int>(&80)?;
                    return Ok(0);
                }

                let buf = UserVAddr::new_nonnull(ifc_buf_ptr)?;
                let mut written = 0usize;
                let avail = ifc_len as usize;

                // eth0 entry (40 bytes)
                if avail >= 40 {
                    let mut ifreq = [0u8; 40];
                    ifreq[..4].copy_from_slice(b"eth0");
                    // sockaddr_in at offset 16
                    ifreq[16] = 2; // AF_INET
                    let (addr, _) = get_ipv4_addr();
                    ifreq[20..24].copy_from_slice(&addr);
                    buf.add(written).write_bytes(&ifreq)?;
                    written += 40;
                }

                // lo entry (40 bytes)
                if avail >= written + 40 {
                    let mut ifreq = [0u8; 40];
                    ifreq[..2].copy_from_slice(b"lo");
                    ifreq[16] = 2; // AF_INET
                    ifreq[20..24].copy_from_slice(&[127, 0, 0, 1]);
                    buf.add(written).write_bytes(&ifreq)?;
                    written += 40;
                }

                // Write back actual length
                arg.write::<c_int>(&(written as c_int))?;
                Ok(0)
            }

            _ => {
                debug_warn!("net_ioctl: unhandled cmd 0x{:x}", cmd);
                Err(Errno::ENOTTY.into())
            }
        }
    }
}
