// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::{
    fs::{
        inode::{FileLike, PollStatus},
        opened_file::OpenOptions,
    },
    net::{socket::SockAddr, RecvFromFlags},
    result::{Errno, Result},
    user_buffer::UserBuffer,
    user_buffer::{UserBufReader, UserBufWriter, UserBufferMut},
};
use alloc::{collections::BTreeSet, sync::Arc, vec::Vec};
use core::{
    cmp::min,
    fmt,
    sync::atomic::{AtomicUsize, Ordering},
};
use crossbeam::atomic::AtomicCell;
use kevlar_platform::spinlock::{SpinLock, SpinLockGuard};
use smoltcp::iface::SocketHandle;
use smoltcp::socket::tcp;
use smoltcp::wire::{IpAddress, IpEndpoint, IpListenEndpoint, Ipv4Address};

use super::{process_packets, INTERFACE, SOCKETS, SOCKET_WAIT_QUEUE};

const BACKLOG_MAX: usize = 8;
static INUSE_ENDPOINTS: SpinLock<BTreeSet<u16>> = SpinLock::new(BTreeSet::new());
static PASSIVE_OPENS_TOTAL: AtomicUsize = AtomicUsize::new(0);
static WRITTEN_BYTES_TOTAL: AtomicUsize = AtomicUsize::new(0);
static READ_BYTES_TOTAL: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug)]
pub struct Stats {
    pub passive_opens_total: usize,
    pub written_bytes_total: usize,
    pub read_bytes_total: usize,
}

pub fn read_tcp_stats() -> Stats {
    Stats {
        passive_opens_total: PASSIVE_OPENS_TOTAL.load(Ordering::SeqCst),
        written_bytes_total: WRITTEN_BYTES_TOTAL.load(Ordering::SeqCst),
        read_bytes_total: READ_BYTES_TOTAL.load(Ordering::SeqCst),
    }
}

/// Looks for an accept'able socket in the backlog.
fn get_ready_backlog_index(
    sockets: &mut smoltcp::iface::SocketSet,
    backlogs: &[Arc<TcpSocket>],
) -> Option<usize> {
    backlogs.iter().position(|sock| {
        let smol_socket: &tcp::Socket = sockets.get(sock.handle);
        smol_socket.may_recv() || smol_socket.may_send()
    })
}

pub struct TcpSocket {
    handle: SocketHandle,
    local_endpoint: AtomicCell<Option<IpEndpoint>>,
    backlogs: SpinLock<Vec<Arc<TcpSocket>>>,
    num_backlogs: AtomicCell<usize>,
    // Per-socket options.
    reuseaddr: AtomicCell<bool>,
    keepalive: AtomicCell<bool>,
    nodelay: AtomicCell<bool>,
    rcvtimeo_us: AtomicCell<u64>,
    sndtimeo_us: AtomicCell<u64>,
}

impl TcpSocket {
    pub fn new() -> Arc<TcpSocket> {
        let rx_buffer = tcp::SocketBuffer::new(vec![0; 4096]);
        let tx_buffer = tcp::SocketBuffer::new(vec![0; 4096]);
        let inner = tcp::Socket::new(rx_buffer, tx_buffer);
        let handle = SOCKETS.lock().add(inner);
        Arc::new(TcpSocket {
            handle,
            local_endpoint: AtomicCell::new(None),
            backlogs: SpinLock::new(Vec::new()),
            num_backlogs: AtomicCell::new(0),
            reuseaddr: AtomicCell::new(false),
            keepalive: AtomicCell::new(false),
            nodelay: AtomicCell::new(false),
            rcvtimeo_us: AtomicCell::new(0),
            sndtimeo_us: AtomicCell::new(0),
        })
    }

    // ── Socket option accessors ──

    pub fn reuseaddr(&self) -> bool { self.reuseaddr.load() }
    pub fn set_reuseaddr(&self, val: bool) { self.reuseaddr.store(val); }

    pub fn keepalive(&self) -> bool { self.keepalive.load() }
    pub fn set_keepalive(&self, val: bool) {
        self.keepalive.store(val);
        let interval = if val {
            Some(smoltcp::time::Duration::from_secs(75))
        } else {
            None
        };
        SOCKETS.lock().get_mut::<tcp::Socket>(self.handle).set_keep_alive(interval);
    }

    pub fn nodelay(&self) -> bool { self.nodelay.load() }
    pub fn set_nodelay(&self, val: bool) {
        self.nodelay.store(val);
        SOCKETS.lock().get_mut::<tcp::Socket>(self.handle).set_nagle_enabled(!val);
    }

    pub fn rcvtimeo_us(&self) -> u64 { self.rcvtimeo_us.load() }
    pub fn set_rcvtimeo(&self, us: u64) { self.rcvtimeo_us.store(us); }

    pub fn sndtimeo_us(&self) -> u64 { self.sndtimeo_us.load() }
    pub fn set_sndtimeo(&self, us: u64) { self.sndtimeo_us.store(us); }

    fn refill_backlog_sockets(
        &self,
        backlogs: &mut SpinLockGuard<'_, Vec<Arc<TcpSocket>>>,
    ) -> Result<()> {
        let local_endpoint = match self.local_endpoint.load() {
            Some(local_endpoint) => local_endpoint,
            None => return Err(Errno::EINVAL.into()),
        };

        let listen_endpoint = IpListenEndpoint {
            addr: Some(local_endpoint.addr),
            port: local_endpoint.port,
        };

        for _ in 0..(self.num_backlogs.load() - backlogs.len()) {
            let socket = TcpSocket::new();
            SOCKETS
                .lock()
                .get_mut::<tcp::Socket>(socket.handle)
                .listen(listen_endpoint)
                .map_err(|_| Errno::EADDRINUSE)?;
            backlogs.push(socket);
        }

        Ok(())
    }
}

impl FileLike for TcpSocket {
    fn listen(&self, backlog: i32) -> Result<()> {
        let mut backlogs = self.backlogs.lock();

        let new_num_backlogs = min(backlog as usize, BACKLOG_MAX);
        backlogs.truncate(new_num_backlogs);
        self.num_backlogs.store(new_num_backlogs);

        self.refill_backlog_sockets(&mut backlogs)
    }

    fn accept(&self, _options: &OpenOptions) -> Result<(Arc<dyn FileLike>, SockAddr)> {
        SOCKET_WAIT_QUEUE.sleep_signalable_until(|| {
            let mut sockets = SOCKETS.lock();
            let mut backlogs = self.backlogs.lock();
            match get_ready_backlog_index(&mut sockets, &backlogs) {
                Some(index) => {
                    // Pop the client socket and add a new socket into the backlog.
                    let socket = backlogs.remove(index);
                    drop(sockets);
                    self.refill_backlog_sockets(&mut backlogs)?;

                    // Extract the remote endpoint.
                    let sockets_lock = SOCKETS.lock();
                    let smol_socket: &tcp::Socket = sockets_lock.get(socket.handle);

                    PASSIVE_OPENS_TOTAL.fetch_add(1, Ordering::SeqCst);

                    let remote = smol_socket
                        .remote_endpoint()
                        .unwrap_or(IpEndpoint {
                            addr: IpAddress::Ipv4(Ipv4Address::UNSPECIFIED),
                            port: 0,
                        });

                    Ok(Some((
                        socket as Arc<dyn FileLike>,
                        super::socket::endpoint_to_sockaddr(remote),
                    )))
                }
                None => {
                    // No accept'able sockets.
                    Ok(None)
                }
            }
        })
    }

    fn bind(&self, sockaddr: SockAddr) -> Result<()> {
        let mut endpoint = super::sockaddr_to_endpoint(sockaddr)?;
        let mut inuse = INUSE_ENDPOINTS.lock();
        if endpoint.port == 0 {
            // Assign an ephemeral port from the dynamic range (49152-65535).
            let mut port: u16 = 49152;
            while inuse.contains(&port) {
                if port == u16::MAX {
                    return Err(Errno::EAGAIN.into());
                }
                port += 1;
            }
            endpoint.port = port;
        } else if inuse.contains(&endpoint.port) && !self.reuseaddr.load() {
            return Err(Errno::EADDRINUSE.into());
        }
        inuse.insert(endpoint.port);
        self.local_endpoint.store(Some(endpoint));
        Ok(())
    }

    fn shutdown(&self, _how: super::ShutdownHow) -> Result<()> {
        SOCKETS
            .lock()
            .get_mut::<tcp::Socket>(self.handle)
            .close();

        process_packets();
        Ok(())
    }

    fn getsockname(&self) -> Result<SockAddr> {
        let sockets = SOCKETS.lock();
        let smol_socket: &tcp::Socket = sockets.get(self.handle);
        let endpoint = smol_socket.local_endpoint();

        match endpoint {
            Some(ep) => Ok(super::socket::endpoint_to_sockaddr(ep)),
            None => Err(Errno::ENOTCONN.into()),
        }
    }

    fn getpeername(&self) -> Result<SockAddr> {
        let sockets = SOCKETS.lock();
        let smol_socket: &tcp::Socket = sockets.get(self.handle);
        let endpoint = smol_socket.remote_endpoint();

        match endpoint {
            Some(ep) => Ok(super::socket::endpoint_to_sockaddr(ep)),
            None => Err(Errno::ENOTCONN.into()),
        }
    }

    fn connect(&self, sockaddr: SockAddr, options: &OpenOptions) -> Result<()> {
        let remote_endpoint: IpEndpoint = super::sockaddr_to_endpoint(sockaddr)?;

        // Check if already connecting/connected.
        {
            let sockets = SOCKETS.lock();
            let socket: &tcp::Socket = sockets.get(self.handle);
            match socket.state() {
                tcp::State::Established => return Err(Errno::EISCONN.into()),
                tcp::State::SynSent => return Err(Errno::EALREADY.into()),
                _ => {}
            }
        }

        let mut inuse_endpoints = INUSE_ENDPOINTS.lock();
        let local_endpoint = self.local_endpoint.load().unwrap_or(IpEndpoint {
            addr: IpAddress::Ipv4(Ipv4Address::UNSPECIFIED),
            port: 0,
        });

        let mut local_port = local_endpoint.port;
        if local_port == 0 {
            let mut port = 50000;
            while inuse_endpoints.contains(&port) {
                if port == u16::MAX {
                    return Err(Errno::EAGAIN.into());
                }
                port += 1;
            }
            local_port = port;
        }

        // Resolve 0.0.0.0 → interface IP so SYN goes out with the correct source.
        let local_addr = if local_endpoint.addr.is_unspecified() {
            let iface = INTERFACE.lock();
            iface.ipv4_addr().map(IpAddress::Ipv4).unwrap_or(local_endpoint.addr)
        } else {
            local_endpoint.addr
        };

        let listen_endpoint = IpListenEndpoint {
            addr: Some(local_addr),
            port: local_port,
        };

        {
            let mut iface = INTERFACE.lock();
            let cx = iface.context();
            SOCKETS
                .lock()
                .get_mut::<tcp::Socket>(self.handle)
                .connect(cx, remote_endpoint, listen_endpoint)
                .map_err(|_| Errno::ECONNRESET)?;
        }
        inuse_endpoints.insert(local_port);
        drop(inuse_endpoints);

        // Submit a SYN packet.
        process_packets();

        #[cfg(feature = "ktrace-net")]
        {
            let IpAddress::Ipv4(v4) = remote_endpoint.addr;
            let ip_u32 = u32::from_be_bytes(v4.octets());
            crate::debug::ktrace::trace(crate::debug::ktrace::event::NET_CONNECT,
                0, ip_u32, remote_endpoint.port as u32, 0, 0);
        }

        if options.nonblock {
            return Err(Errno::EINPROGRESS.into());
        }

        // Wait until the connection has been established.
        SOCKET_WAIT_QUEUE.sleep_signalable_until(|| {
            process_packets();
            let sockets = SOCKETS.lock();
            let socket: &tcp::Socket = sockets.get(self.handle);
            match socket.state() {
                tcp::State::Established => Ok(Some(())),
                tcp::State::Closed => Err(Errno::ECONNREFUSED.into()),
                _ => Ok(None),
            }
        })
    }

    fn write(&self, _offset: usize, buf: UserBuffer<'_>, options: &OpenOptions) -> Result<usize> {
        let mut total_len = 0;
        let mut reader = UserBufReader::from(buf);
        let timeout_us = self.sndtimeo_us.load();
        let started_at = if timeout_us > 0 {
            Some(crate::timer::read_monotonic_clock())
        } else {
            None
        };
        loop {
            let copied_len = SOCKETS
                .lock()
                .get_mut::<tcp::Socket>(self.handle)
                .send(|dst| {
                    let copied_len = reader.read_bytes(dst).unwrap_or(0);
                    (copied_len, copied_len)
                });

            process_packets();
            match copied_len {
                Ok(0) if total_len > 0 || options.nonblock => {
                    WRITTEN_BYTES_TOTAL.fetch_add(total_len, Ordering::SeqCst);
                    #[cfg(feature = "ktrace-net")]
                    crate::debug::ktrace::trace(crate::debug::ktrace::event::NET_SEND,
                        0, total_len as u32, total_len as u32, 0, 0);
                    return Ok(total_len);
                }
                Ok(0) => {
                    // Check SO_SNDTIMEO deadline.
                    if let Some(start) = started_at {
                        if (start.elapsed_msecs() as u64) * 1000 >= timeout_us {
                            if total_len > 0 { return Ok(total_len); }
                            return Err(Errno::EAGAIN.into());
                        }
                    }
                    SOCKET_WAIT_QUEUE.sleep_signalable_until(|| {
                        if let Some(start) = started_at {
                            if (start.elapsed_msecs() as u64) * 1000 >= timeout_us {
                                return Err(Errno::EAGAIN.into());
                            }
                        }
                        process_packets();
                        let sockets = SOCKETS.lock();
                        let socket: &tcp::Socket = sockets.get(self.handle);
                        if socket.can_send() {
                            Ok(Some(()))
                        } else if !socket.may_send() {
                            Err(Errno::ECONNRESET.into())
                        } else {
                            Ok(None)
                        }
                    })?;
                }
                Ok(copied_len) => {
                    total_len += copied_len;
                }
                Err(_) => return Err(Errno::ECONNRESET.into()),
            }
        }
    }

    fn read(&self, _offset: usize, buf: UserBufferMut<'_>, options: &OpenOptions) -> Result<usize> {
        let mut writer = UserBufWriter::from(buf);
        let timeout_us = self.rcvtimeo_us.load();
        let started_at = if timeout_us > 0 {
            Some(crate::timer::read_monotonic_clock())
        } else {
            None
        };
        SOCKET_WAIT_QUEUE.sleep_signalable_until(|| {
            // Check SO_RCVTIMEO deadline.
            if let Some(start) = started_at {
                if (start.elapsed_msecs() as u64) * 1000 >= timeout_us {
                    return Err(Errno::EAGAIN.into());
                }
            }

            process_packets();

            let copied_len = SOCKETS
                .lock()
                .get_mut::<tcp::Socket>(self.handle)
                .recv(|src| {
                    let copied_len = writer.write_bytes(src).unwrap_or(0);
                    (copied_len, copied_len)
                });

            match copied_len {
                Ok(0) => {
                    if options.nonblock {
                        Err(Errno::EAGAIN.into())
                    } else {
                        Ok(None)
                    }
                }
                Err(tcp::RecvError::Finished) => Ok(Some(0)),
                Ok(copied_len) => {
                    READ_BYTES_TOTAL.fetch_add(copied_len, Ordering::SeqCst);
                    #[cfg(feature = "ktrace-net")]
                    crate::debug::ktrace::trace(crate::debug::ktrace::event::NET_RECV,
                        0, copied_len as u32, copied_len as u32, 0, 0);
                    Ok(Some(copied_len))
                }
                Err(_) => Err(Errno::ECONNRESET.into()),
            }
        })
    }

    fn sendto(
        &self,
        buf: UserBuffer<'_>,
        sockaddr: Option<SockAddr>,
        options: &OpenOptions,
    ) -> Result<usize> {
        if sockaddr.is_some() {
            return Err(Errno::EINVAL.into());
        }

        self.write(0, buf, options)
    }

    fn recvfrom(
        &self,
        buf: UserBufferMut<'_>,
        _flags: RecvFromFlags,
        options: &OpenOptions,
    ) -> Result<(usize, SockAddr)> {
        Ok((self.read(0, buf, options)?, self.getpeername()?))
    }

    fn poll(&self) -> Result<PollStatus> {
        process_packets();
        let mut status = PollStatus::empty();
        let mut sockets = SOCKETS.lock();
        if get_ready_backlog_index(&mut sockets, &self.backlogs.lock()).is_some() {
            status |= PollStatus::POLLIN;
        }

        let socket: &tcp::Socket = sockets.get(self.handle);
        if socket.can_recv() {
            status |= PollStatus::POLLIN;
        }

        if socket.can_send() {
            status |= PollStatus::POLLOUT;
        }

        // Report POLLIN for EOF: remote sent FIN, read() would return 0
        // immediately. This lets poll/epoll wake the application.
        if !socket.may_recv() && matches!(socket.state(),
            tcp::State::CloseWait | tcp::State::LastAck |
            tcp::State::TimeWait | tcp::State::Closing) {
            status |= PollStatus::POLLIN;
        }

        // Report error for failed nonblocking connect or reset.
        // Skip for listening sockets: their self.handle stays in CLOSED
        // state (only backlog sockets are put in LISTEN state), which is
        // not an error condition.
        if socket.state() == tcp::State::Closed && self.num_backlogs.load() == 0 {
            status |= PollStatus::POLLERR;
        }

        #[cfg(feature = "ktrace-net")]
        crate::debug::ktrace::trace(crate::debug::ktrace::event::NET_POLL,
            0, 0, status.bits() as u32, 0, 0);

        Ok(status)
    }
}

impl Drop for TcpSocket {
    fn drop(&mut self) {
        // Release the bound port from the in-use set.
        if let Some(ep) = self.local_endpoint.load() {
            if ep.port != 0 {
                INUSE_ENDPOINTS.lock().remove(&ep.port);
            }
        }
        SOCKETS.lock().remove(self.handle);
    }
}

impl fmt::Debug for TcpSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TcpSocket").finish()
    }
}
