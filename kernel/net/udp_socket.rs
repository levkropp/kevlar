// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::{
    fs::{
        inode::{FileLike, PollStatus},
        opened_file::OpenOptions,
    },
    result::{Errno, Error, Result},
    user_buffer::UserBuffer,
    user_buffer::{UserBufReader, UserBufWriter, UserBufferMut},
};
use alloc::{collections::BTreeSet, sync::Arc};
use core::fmt;
use kevlar_platform::spinlock::SpinLock;
use smoltcp::iface::SocketHandle;
use smoltcp::socket::udp;
use smoltcp::wire::IpEndpoint;

use super::{process_packets, socket::*, SOCKETS, SOCKET_WAIT_QUEUE};

static INUSE_ENDPOINTS: SpinLock<BTreeSet<u16>> = SpinLock::new(BTreeSet::new());

pub struct UdpSocket {
    handle: SocketHandle,
    peer: SpinLock<Option<IpEndpoint>>,
}

impl UdpSocket {
    pub fn new() -> Arc<UdpSocket> {
        let rx_buffer = udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 64], vec![0; 4096]);
        let tx_buffer = udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 64], vec![0; 4096]);
        let inner = udp::Socket::new(rx_buffer, tx_buffer);
        let handle = SOCKETS.lock().add(inner);
        Arc::new(UdpSocket { handle, peer: SpinLock::new(None) })
    }
}

impl FileLike for UdpSocket {
    fn bind(&self, sockaddr: SockAddr) -> Result<()> {
        let mut endpoint: IpEndpoint = super::sockaddr_to_endpoint(sockaddr)?;
        // TODO: Reject if the endpoint is already in use -- IIUC smoltcp
        //       does not check that.
        let mut inuse_endpoints = INUSE_ENDPOINTS.lock();

        if endpoint.port == 0 {
            // Assign a unused port.
            // TODO: Assign a *random* port instead.
            let mut port = 50000;
            while inuse_endpoints.contains(&port) {
                if port == u16::MAX {
                    return Err(Errno::EAGAIN.into());
                }

                port += 1;
            }
            endpoint.port = port;
        }

        SOCKETS
            .lock()
            .get_mut::<udp::Socket>(self.handle)
            .bind(endpoint)
            .map_err(|_| Errno::EADDRINUSE)?;
        inuse_endpoints.insert(endpoint.port);

        Ok(())
    }

    fn connect(&self, sockaddr: SockAddr, _options: &OpenOptions) -> Result<()> {
        let endpoint = super::sockaddr_to_endpoint(sockaddr)?;
        *self.peer.lock_no_irq() = Some(endpoint);

        // Auto-bind if not yet bound (musl expects connect on UDP to bind).
        let mut sockets = SOCKETS.lock();
        let socket = sockets.get_mut::<udp::Socket>(self.handle);
        if !socket.is_open() {
            let mut inuse_endpoints = INUSE_ENDPOINTS.lock();
            let mut port = 49152;
            while inuse_endpoints.contains(&port) {
                port += 1;
            }
            let bind_ep = IpEndpoint::new(smoltcp::wire::IpAddress::v4(0, 0, 0, 0), port);
            socket.bind(bind_ep).map_err(|_| Errno::EADDRINUSE)?;
            inuse_endpoints.insert(port);
        }
        Ok(())
    }

    fn sendto(
        &self,
        buf: UserBuffer<'_>,
        sockaddr: Option<SockAddr>,
        _options: &OpenOptions,
    ) -> Result<usize> {
        let endpoint = match sockaddr {
            Some(sa) => super::sockaddr_to_endpoint(sa)?,
            None => self.peer.lock_no_irq()
                .ok_or_else(|| Error::new(Errno::EINVAL))?,
        };

        // Flush pending network events (especially DHCP Ack) so that the
        // interface IP is up-to-date before we check it for source-address
        // rebinding.  Without this, the first sendto after boot may see
        // 0.0.0.0 and skip the rebind, causing replies to be dropped.
        process_packets();

        let mut sockets = SOCKETS.lock();

        // If the socket is bound to INADDR_ANY (0.0.0.0), rebind to the
        // interface's actual IP.  smoltcp uses the socket's bound address as
        // the IP source; 0.0.0.0 goes out on the wire verbatim, causing the
        // reply to be addressed to 0.0.0.0 which the interface then drops.
        {
            let socket = sockets.get_mut::<udp::Socket>(self.handle);
            if let Some(ep) = socket.endpoint().addr {
                if ep.is_unspecified() {
                    let iface = super::INTERFACE.lock();
                    if let Some(cidr) = iface.ip_addrs().first() {
                        let real_ip = match cidr {
                            smoltcp::wire::IpCidr::Ipv4(c) => {
                                smoltcp::wire::IpAddress::Ipv4(c.address())
                            }
                            #[allow(unreachable_patterns)]
                            _ => ep,
                        };
                        if !real_ip.is_unspecified() {
                            let port = socket.endpoint().port;
                            socket.close();
                            let _ = socket.bind(IpEndpoint::new(real_ip, port));
                        }
                    }
                }
            }
        }

        let socket = sockets.get_mut::<udp::Socket>(self.handle);
        let mut reader = UserBufReader::from(buf);
        let dst = socket
            .send(reader.remaining_len(), endpoint)
            .map_err(|_| Errno::ENOBUFS)?;
        let copied_len = reader.read_bytes(dst)?;

        drop(sockets);

        // Clear the ARP-sent flag before driving the stack.  If process_packets()
        // triggers an ARP request (cold neighbor cache), ARP_SENT will be set.
        super::ARP_SENT.store(false, core::sync::atomic::Ordering::Relaxed);
        process_packets();

        // If an ARP request was just sent, the UDP packet is sitting in smoltcp's
        // single-slot ARP pending cache.  A second sendto() before the ARP reply
        // arrives would replace it, silently dropping this packet.
        //
        // Spin briefly (up to 1ms) with interrupts enabled so the ARP reply can
        // arrive via virtio-net IRQ, then re-drive the stack to flush the pending
        // packet before we return.
        if super::ARP_SENT.load(core::sync::atomic::Ordering::Relaxed) {
            let start = kevlar_platform::arch::tsc::nanoseconds_since_boot();
            loop {
                if !super::RX_PACKET_QUEUE.lock().is_empty() {
                    process_packets();
                    break;
                }
                if kevlar_platform::arch::tsc::nanoseconds_since_boot() - start > 1_000_000 {
                    // Timeout — give up; poll timeout + DNS retry will recover.
                    break;
                }
                core::hint::spin_loop();
            }
        }

        Ok(copied_len)
    }

    fn recvfrom(
        &self,
        buf: UserBufferMut<'_>,
        _flags: RecvFromFlags,
        options: &OpenOptions,
    ) -> Result<(usize, SockAddr)> {
        let mut writer = UserBufWriter::from(buf);
        SOCKET_WAIT_QUEUE.sleep_signalable_until(|| {
            process_packets();
            let mut sockets = SOCKETS.lock();
            let socket = sockets.get_mut::<udp::Socket>(self.handle);
            match socket.recv() {
                Ok((payload, meta)) => {
                    writer.write_bytes(payload)?;
                    Ok(Some((writer.written_len(), super::socket::endpoint_to_sockaddr(meta.endpoint))))
                }
                Err(udp::RecvError::Exhausted) if options.nonblock => Err(Errno::EAGAIN.into()),
                Err(udp::RecvError::Exhausted) => {
                    // The receive buffer is empty. Try again later...
                    Ok(None)
                }
                Err(_) => Err(Errno::EINVAL.into()),
            }
        })
    }

    fn poll(&self) -> Result<PollStatus> {
        process_packets();
        let sockets = SOCKETS.lock();
        let socket: &udp::Socket = sockets.get(self.handle);

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

impl fmt::Debug for UdpSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UdpSocket").finish()
    }
}
