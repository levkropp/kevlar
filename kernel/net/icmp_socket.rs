// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! ICMP "ping socket" — AF_INET + SOCK_DGRAM + IPPROTO_ICMP.
//!
//! BusyBox ping uses this Linux feature instead of raw sockets.
//! We use smoltcp's icmp::Socket to send echo requests and receive replies.
use crate::{
    fs::{
        inode::{FileLike, PollStatus},
        opened_file::OpenOptions,
    },
    net::socket::*,
    result::{Errno, Error, Result},
    user_buffer::{UserBufReader, UserBufWriter, UserBuffer, UserBufferMut},
};
use alloc::sync::Arc;
use core::fmt;
use kevlar_platform::spinlock::SpinLock;
use smoltcp::iface::SocketHandle;
use smoltcp::socket::icmp;
use smoltcp::wire::{IpAddress, Ipv4Address};

use super::{process_packets, SOCKETS, SOCKET_WAIT_QUEUE};

/// The remote address this ICMP socket is "connected" to via sendto dest.
pub struct IcmpSocket {
    handle: SocketHandle,
    /// Bound ICMP ident (acts like a port for demuxing).
    ident: SpinLock<u16>,
}

impl IcmpSocket {
    pub fn new() -> Arc<IcmpSocket> {
        let rx_buffer = icmp::PacketBuffer::new(
            vec![icmp::PacketMetadata::EMPTY; 16],
            vec![0; 4096],
        );
        let tx_buffer = icmp::PacketBuffer::new(
            vec![icmp::PacketMetadata::EMPTY; 16],
            vec![0; 4096],
        );
        let inner = icmp::Socket::new(rx_buffer, tx_buffer);
        let handle = SOCKETS.lock().add(inner);
        Arc::new(IcmpSocket {
            handle,
            ident: SpinLock::new(0),
        })
    }

    fn ensure_bound(&self) -> Result<u16> {
        let mut ident = self.ident.lock();
        if *ident == 0 {
            // Auto-bind with a pseudo-random ident.
            let mut buf = [0u8; 2];
            kevlar_platform::random::rdrand_fill(&mut buf);
            let id = u16::from_ne_bytes(buf) | 0x100; // ensure nonzero
            *ident = id;
            SOCKETS
                .lock()
                .get_mut::<icmp::Socket>(self.handle)
                .bind(icmp::Endpoint::Ident(id))
                .map_err(|_| Errno::EADDRINUSE)?;
        }
        Ok(*ident)
    }
}

impl FileLike for IcmpSocket {
    fn bind(&self, _sockaddr: SockAddr) -> Result<()> {
        // BusyBox ping doesn't call bind; auto-bind on first send.
        Ok(())
    }

    fn sendto(
        &self,
        buf: UserBuffer<'_>,
        sockaddr: Option<SockAddr>,
        _options: &OpenOptions,
    ) -> Result<usize> {
        let dest = sockaddr.ok_or_else(|| Error::new(Errno::EINVAL))?;
        let endpoint = super::sockaddr_to_endpoint(dest)?;

        self.ensure_bound()?;

        // Read the ICMP payload from userspace.
        // BusyBox sends a raw ICMP packet (type, code, checksum, id, seq, data).
        let mut data = alloc::vec![0u8; buf.len()];
        let mut reader = UserBufReader::from(buf);
        let len = reader.read_bytes(&mut data)?;

        // Send the raw ICMP packet bytes via smoltcp.
        let mut sockets = SOCKETS.lock();
        let socket = sockets.get_mut::<icmp::Socket>(self.handle);
        let tx_buf = socket
            .send(len, endpoint.addr)
            .map_err(|_| Errno::ENOBUFS)?;
        tx_buf[..len].copy_from_slice(&data[..len]);

        drop(sockets);
        process_packets();
        Ok(len)
    }

    fn recvfrom(
        &self,
        buf: UserBufferMut<'_>,
        _flags: RecvFromFlags,
        options: &OpenOptions,
    ) -> Result<(usize, SockAddr)> {
        self.ensure_bound()?;
        let mut writer = UserBufWriter::from(buf);
        SOCKET_WAIT_QUEUE.sleep_signalable_until(|| {
            let mut sockets = SOCKETS.lock();
            let socket = sockets.get_mut::<icmp::Socket>(self.handle);
            match socket.recv() {
                Ok((payload, addr)) => {
                    writer.write_bytes(payload)?;
                    let src_addr = match addr {
                        IpAddress::Ipv4(v4) => v4,
                        #[allow(unreachable_patterns)]
                        _ => Ipv4Address::UNSPECIFIED,
                    };
                    let sockaddr = SockAddr::In(SockAddrIn {
                        family: AF_INET as u16,
                        port: [0, 0],
                        addr: src_addr.octets(),
                        zero: [0; 8],
                    });
                    Ok(Some((writer.written_len(), sockaddr)))
                }
                Err(icmp::RecvError::Exhausted) if options.nonblock => {
                    Err(Errno::EAGAIN.into())
                }
                Err(icmp::RecvError::Exhausted) => Ok(None),
                Err(_) => Err(Errno::EINVAL.into()),
            }
        })
    }

    fn poll(&self) -> Result<PollStatus> {
        let sockets = SOCKETS.lock();
        let socket: &icmp::Socket = sockets.get(self.handle);

        let mut status = PollStatus::empty();
        if socket.can_recv() {
            status |= PollStatus::POLLIN;
        }
        if socket.can_send() {
            status |= PollStatus::POLLOUT;
        }

        Ok(status)
    }
}

impl Drop for IcmpSocket {
    fn drop(&mut self) {
        SOCKETS.lock().remove(self.handle);
    }
}

impl fmt::Debug for IcmpSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IcmpSocket").finish()
    }
}
