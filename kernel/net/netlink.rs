// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Minimal NETLINK_ROUTE socket implementation.
//!
//! Supports the three operations Alpine's `ip` tool needs:
//! - RTM_NEWLINK: bring interfaces up/down
//! - RTM_NEWADDR: assign IPv4 addresses
//! - RTM_NEWROUTE: add default routes
//!
//! Messages are parsed from sendto, handled immediately, and ACK responses
//! are queued for the next recvfrom.

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicU32, Ordering};

use kevlar_platform::spinlock::SpinLock;
use kevlar_vfs::{
    inode::{FileLike, OpenOptions, PollStatus},
    result::{Errno, Error, Result},
    socket_types::*,
    stat::Stat,
    user_buffer::{UserBufReader, UserBufWriter, UserBuffer, UserBufferMut},
};

use super::INTERFACE;

// ── Netlink constants ─────────────────────────────────────────────

const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;

const RTM_NEWLINK: u16 = 16;
const RTM_SETLINK: u16 = 17;
const RTM_GETLINK: u16 = 18;
const RTM_NEWADDR: u16 = 20;
const RTM_GETADDR: u16 = 22;
const RTM_NEWROUTE: u16 = 24;
const RTM_GETROUTE: u16 = 26;

const NLM_F_ACK: u16 = 4;
const NLM_F_DUMP: u16 = 0x300;

// rtattr types for RTM_NEWADDR
const IFA_ADDRESS: u16 = 1;
const IFA_LOCAL: u16 = 2;

// rtattr types for RTM_NEWROUTE
const RTA_GATEWAY: u16 = 5;

// Sizes
const NLMSGHDR_SIZE: usize = 16;
const IFINFOMSG_SIZE: usize = 16;
const IFADDRMSG_SIZE: usize = 8;
const RTMSG_SIZE: usize = 12;

// ── Byte parsing helpers ──────────────────────────────────────────

fn read_u16(data: &[u8], off: usize) -> u16 {
    u16::from_ne_bytes([data[off], data[off + 1]])
}

fn read_u32(data: &[u8], off: usize) -> u32 {
    u32::from_ne_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
}

fn read_i32(data: &[u8], off: usize) -> i32 {
    i32::from_ne_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
}

fn write_u16(buf: &mut [u8], off: usize, val: u16) {
    let b = val.to_ne_bytes();
    buf[off] = b[0];
    buf[off + 1] = b[1];
}

fn write_u32(buf: &mut [u8], off: usize, val: u32) {
    let b = val.to_ne_bytes();
    buf[off] = b[0];
    buf[off + 1] = b[1];
    buf[off + 2] = b[2];
    buf[off + 3] = b[3];
}

fn write_i32(buf: &mut [u8], off: usize, val: i32) {
    let b = val.to_ne_bytes();
    buf[off] = b[0];
    buf[off + 1] = b[1];
    buf[off + 2] = b[2];
    buf[off + 3] = b[3];
}

fn nlmsg_align(len: usize) -> usize {
    (len + 3) & !3
}

// ── NetlinkSocket ─────────────────────────────────────────────────

pub struct NetlinkSocket {
    response_queue: SpinLock<VecDeque<Vec<u8>>>,
    port_id: AtomicU32,
}

impl NetlinkSocket {
    pub fn new() -> Arc<Self> {
        Arc::new(NetlinkSocket {
            response_queue: SpinLock::new(VecDeque::new()),
            port_id: AtomicU32::new(0),
        })
    }

    /// Parse and handle one or more netlink messages from raw bytes.
    fn handle_messages(&self, data: &[u8]) -> Result<()> {
        let mut offset = 0;
        while offset + NLMSGHDR_SIZE <= data.len() {
            let msg_len = read_u32(data, offset) as usize;
            if msg_len < NLMSGHDR_SIZE || offset + msg_len > data.len() {
                break;
            }

            let msg_type = read_u16(data, offset + 4);
            let msg_flags = read_u16(data, offset + 6);
            let msg_seq = read_u32(data, offset + 8);
            let msg_pid = read_u32(data, offset + 12);

            let payload = &data[offset + NLMSGHDR_SIZE..offset + msg_len];

            let result = match msg_type {
                RTM_NEWLINK | RTM_SETLINK => self.handle_newlink(payload),
                RTM_NEWADDR => self.handle_newaddr(payload),
                RTM_NEWROUTE => self.handle_newroute(payload),
                RTM_GETLINK => {
                    // Return a single eth0 interface entry + DONE.
                    self.queue_getlink_response(msg_seq, self.port_id.load(Ordering::Relaxed));
                    Ok(())
                }
                RTM_GETADDR | RTM_GETROUTE => {
                    // Dump requests: return NLMSG_DONE (empty).
                    self.queue_done(msg_seq, self.port_id.load(Ordering::Relaxed));
                    Ok(())
                }
                _ => {
                    // Unknown message type — accept silently.
                    Ok(())
                }
            };

            // Queue ACK if requested (NLM_F_ACK) or always for set operations.
            // BusyBox ip expects an ACK for RTM_SETLINK etc.
            if msg_flags & NLM_F_ACK != 0 || matches!(msg_type, RTM_SETLINK | RTM_NEWLINK | RTM_NEWADDR | RTM_NEWROUTE) {
                let error = match &result {
                    Ok(()) => 0,
                    Err(e) => -(e.errno() as i32),
                };
                self.queue_ack(msg_seq, self.port_id.load(Ordering::Relaxed), error,
                    &data[offset..offset + core::cmp::min(NLMSGHDR_SIZE, msg_len)]);
            }

            offset += nlmsg_align(msg_len);
        }
        Ok(())
    }

    /// RTM_NEWLINK: bring an interface up or down.
    fn handle_newlink(&self, payload: &[u8]) -> Result<()> {
        if payload.len() < IFINFOMSG_SIZE {
            return Err(Error::new(Errno::EINVAL));
        }
        // ifinfomsg fields:
        // [0] ifi_family, [1] pad, [2-3] ifi_type, [4-7] ifi_index,
        // [8-11] ifi_flags, [12-15] ifi_change
        // Interface is always "up" in smoltcp — accept silently.
        Ok(())
    }

    /// RTM_NEWADDR: assign an IP address to an interface.
    fn handle_newaddr(&self, payload: &[u8]) -> Result<()> {
        if payload.len() < IFADDRMSG_SIZE {
            return Err(Error::new(Errno::EINVAL));
        }
        // ifaddrmsg: [0] family, [1] prefixlen, [2] flags, [3] scope, [4-7] index
        let family = payload[0];
        let prefix_len = payload[1];

        if family != AF_INET as u8 {
            return Ok(()); // Only IPv4 supported
        }

        // Parse rtattrs after ifaddrmsg to find the address.
        let mut ip_addr: Option<[u8; 4]> = None;
        let attrs = &payload[IFADDRMSG_SIZE..];
        let mut off = 0;
        while off + 4 <= attrs.len() {
            let rta_len = read_u16(attrs, off) as usize;
            if rta_len < 4 || off + rta_len > attrs.len() {
                break;
            }
            let rta_type = read_u16(attrs, off + 2);
            let rta_data = &attrs[off + 4..off + rta_len];

            if (rta_type == IFA_LOCAL || rta_type == IFA_ADDRESS) && rta_data.len() >= 4 {
                ip_addr = Some([rta_data[0], rta_data[1], rta_data[2], rta_data[3]]);
            }

            off += nlmsg_align(rta_len);
        }

        if let Some(addr) = ip_addr {
            use smoltcp::wire::{IpCidr, Ipv4Address, Ipv4Cidr};
            let ipv4 = Ipv4Address::from(addr);
            let cidr = Ipv4Cidr::new(ipv4, prefix_len);
            INTERFACE.lock().update_ip_addrs(|addrs| {
                // Set the first address slot to the new address.
                if let Some(slot) = addrs.iter_mut().next() {
                    *slot = IpCidr::Ipv4(cidr);
                }
            });
            info!("netlink: configured {}/{}", ipv4, prefix_len);
        }

        Ok(())
    }

    /// RTM_NEWROUTE: add a route (typically the default gateway).
    fn handle_newroute(&self, payload: &[u8]) -> Result<()> {
        if payload.len() < RTMSG_SIZE {
            return Err(Error::new(Errno::EINVAL));
        }
        // rtmsg: [0] family, [1] dst_len, [2] src_len, [3] tos,
        // [4] table, [5] protocol, [6] scope, [7] type, [8-11] flags
        let family = payload[0];
        if family != AF_INET as u8 {
            return Ok(());
        }

        // Parse rtattrs to find gateway.
        let mut gateway: Option<[u8; 4]> = None;
        let attrs = &payload[RTMSG_SIZE..];
        let mut off = 0;
        while off + 4 <= attrs.len() {
            let rta_len = read_u16(attrs, off) as usize;
            if rta_len < 4 || off + rta_len > attrs.len() {
                break;
            }
            let rta_type = read_u16(attrs, off + 2);
            let rta_data = &attrs[off + 4..off + rta_len];

            if rta_type == RTA_GATEWAY && rta_data.len() >= 4 {
                gateway = Some([rta_data[0], rta_data[1], rta_data[2], rta_data[3]]);
            }

            off += nlmsg_align(rta_len);
        }

        if let Some(gw) = gateway {
            use smoltcp::wire::Ipv4Address;
            let gw_addr = Ipv4Address::from(gw);
            INTERFACE.lock().routes_mut()
                .add_default_ipv4_route(gw_addr)
                .map_err(|_| Error::new(Errno::ENOMEM))?;
            info!("netlink: default route via {}", gw_addr);
        }

        Ok(())
    }

    /// Queue a NLMSG_ERROR response (error=0 means ACK/success).
    fn queue_ack(&self, seq: u32, pid: u32, error: i32, orig_hdr: &[u8]) {
        // Response: nlmsghdr(16) + error(4) + original_header(16) = 36 bytes
        let orig_len = core::cmp::min(orig_hdr.len(), NLMSGHDR_SIZE);
        let resp_len = NLMSGHDR_SIZE + 4 + orig_len;
        let mut resp = vec![0u8; resp_len];
        write_u32(&mut resp, 0, resp_len as u32); // nlmsg_len
        write_u16(&mut resp, 4, NLMSG_ERROR);     // nlmsg_type
        write_u16(&mut resp, 6, 0);               // nlmsg_flags
        write_u32(&mut resp, 8, seq);              // nlmsg_seq
        write_u32(&mut resp, 12, pid);             // nlmsg_pid
        write_i32(&mut resp, 16, error);           // error code
        resp[20..20 + orig_len].copy_from_slice(&orig_hdr[..orig_len]);
        self.response_queue.lock().push_back(resp);
    }

    /// Queue a RTM_NEWLINK response for eth0 (for RTM_GETLINK dumps).
    fn queue_getlink_response(&self, seq: u32, pid: u32) {
        // Build: nlmsghdr + ifinfomsg + IFLA_IFNAME("eth0")
        let ifname = b"eth0\0";
        let ifname_attr_len = nlmsg_align(4 + ifname.len()); // rtattr hdr + name
        let msg_len = NLMSGHDR_SIZE + IFINFOMSG_SIZE + ifname_attr_len;
        let mut msg = vec![0u8; msg_len];

        // nlmsghdr
        write_u32(&mut msg, 0, msg_len as u32);
        write_u16(&mut msg, 4, RTM_NEWLINK);
        write_u16(&mut msg, 6, 2); // NLM_F_MULTI
        write_u32(&mut msg, 8, seq);
        write_u32(&mut msg, 12, pid);

        // ifinfomsg: family=0, type=ARPHRD_ETHER(1), index=1, flags=UP|RUNNING
        let ifi_off = NLMSGHDR_SIZE;
        msg[ifi_off] = 0; // ifi_family
        write_u16(&mut msg, ifi_off + 2, 1); // ifi_type = ARPHRD_ETHER
        write_i32(&mut msg, ifi_off + 4, 1); // ifi_index = 1 (eth0)
        write_u32(&mut msg, ifi_off + 8, 0x41); // ifi_flags = IFF_UP | IFF_RUNNING
        write_u32(&mut msg, ifi_off + 12, 0xFFFFFFFF); // ifi_change

        // IFLA_IFNAME attr
        let attr_off = NLMSGHDR_SIZE + IFINFOMSG_SIZE;
        write_u16(&mut msg, attr_off, (4 + ifname.len()) as u16); // rta_len
        write_u16(&mut msg, attr_off + 2, 3); // IFLA_IFNAME = 3
        msg[attr_off + 4..attr_off + 4 + ifname.len()].copy_from_slice(ifname);

        self.response_queue.lock().push_back(msg);

        // Also queue NLMSG_DONE
        self.queue_done(seq, pid);
    }

    /// Queue a NLMSG_DONE response (for dump requests).
    fn queue_done(&self, seq: u32, pid: u32) {
        let mut resp = vec![0u8; NLMSGHDR_SIZE];
        write_u32(&mut resp, 0, NLMSGHDR_SIZE as u32);
        write_u16(&mut resp, 4, NLMSG_DONE);
        write_u16(&mut resp, 6, 2); // NLM_F_MULTI
        write_u32(&mut resp, 8, seq);
        write_u32(&mut resp, 12, pid);
        self.response_queue.lock().push_back(resp);
    }
}

impl fmt::Debug for NetlinkSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NetlinkSocket(pid={})", self.port_id.load(Ordering::Relaxed))
    }
}

impl FileLike for NetlinkSocket {
    fn bind(&self, sockaddr: SockAddr) -> Result<()> {
        if let SockAddr::Nl(nl) = sockaddr {
            let pid = if nl.pid == 0 {
                crate::process::current_process().pid().as_i32() as u32
            } else {
                nl.pid
            };
            self.port_id.store(pid, Ordering::Relaxed);
        }
        Ok(())
    }

    fn getsockname(&self) -> Result<SockAddr> {
        Ok(SockAddr::Nl(SockAddrNl {
            family: AF_NETLINK as u16,
            pad: 0,
            pid: self.port_id.load(Ordering::Relaxed),
            groups: 0,
        }))
    }

    fn sendto(
        &self,
        buf: UserBuffer<'_>,
        _sockaddr: Option<SockAddr>,
        _options: &OpenOptions,
    ) -> Result<usize> {
        let len = buf.len();
        let mut data = vec![0u8; len];
        let mut reader = UserBufReader::from(buf);
        reader.read_bytes(&mut data)?;
        self.handle_messages(&data)?;
        Ok(len)
    }

    fn recvfrom(
        &self,
        buf: UserBufferMut<'_>,
        _flags: RecvFromFlags,
        _options: &OpenOptions,
    ) -> Result<(usize, SockAddr)> {
        let response = self.response_queue.lock().pop_front();
        match response {
            Some(data) => {
                let copy_len = core::cmp::min(data.len(), buf.len());
                let mut writer = UserBufWriter::from(buf);
                writer.write_bytes(&data[..copy_len])?;
                let nl_addr = SockAddr::Nl(SockAddrNl {
                    family: AF_NETLINK as u16,
                    pad: 0,
                    pid: 0, // from kernel
                    groups: 0,
                });
                Ok((copy_len, nl_addr))
            }
            None => Err(Error::new(Errno::EAGAIN)),
        }
    }

    fn write(&self, _offset: usize, buf: UserBuffer<'_>, options: &OpenOptions) -> Result<usize> {
        self.sendto(buf, None, options)
    }

    fn read(&self, _offset: usize, buf: UserBufferMut<'_>, options: &OpenOptions) -> Result<usize> {
        match self.recvfrom(buf, RecvFromFlags::empty(), options) {
            Ok((n, _)) => Ok(n),
            Err(e) => Err(e),
        }
    }

    fn poll(&self) -> Result<PollStatus> {
        let queue = self.response_queue.lock();
        let mut status = PollStatus::POLLOUT;
        if !queue.is_empty() {
            status |= PollStatus::POLLIN;
        }
        Ok(status)
    }

    fn stat(&self) -> Result<Stat> {
        Ok(Stat::zeroed())
    }
}
