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
}

impl UdpSocket {
    pub fn new() -> Arc<UdpSocket> {
        let rx_buffer = udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 64], vec![0; 4096]);
        let tx_buffer = udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 64], vec![0; 4096]);
        let inner = udp::Socket::new(rx_buffer, tx_buffer);
        let handle = SOCKETS.lock().add(inner);
        Arc::new(UdpSocket { handle })
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

    fn sendto(
        &self,
        buf: UserBuffer<'_>,
        sockaddr: Option<SockAddr>,
        _options: &OpenOptions,
    ) -> Result<usize> {
        let endpoint = super::sockaddr_to_endpoint(
            sockaddr.ok_or_else(|| Error::new(Errno::EINVAL))?,
        )?;
        let mut sockets = SOCKETS.lock();
        let socket = sockets.get_mut::<udp::Socket>(self.handle);
        let mut reader = UserBufReader::from(buf);
        let dst = socket
            .send(reader.remaining_len(), endpoint)
            .map_err(|_| Errno::ENOBUFS)?;
        let copied_len = reader.read_bytes(dst)?;

        drop(sockets);
        process_packets();
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
