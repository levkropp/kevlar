// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! AF_UNIX (Unix domain) socket implementation.
//!
//! Supports SOCK_STREAM named sockets: bind to a path, listen, accept, connect,
//! bidirectional byte-stream I/O with poll support, and SCM_RIGHTS fd passing
//! via sendmsg/recvmsg ancillary data.
//!
//! Provenance: Own (Linux unix(7), cmsg(3) man pages).
use core::fmt;
use core::sync::atomic::{AtomicBool, Ordering};

use alloc::collections::VecDeque;
use alloc::sync::Arc;

use kevlar_platform::spinlock::SpinLock;
use kevlar_utils::ring_buffer::RingBuffer;

use crate::fs::inode::{FileLike, PollStatus};
use crate::fs::opened_file::{OpenOptions, OpenedFile};
use crate::net::socket::SockAddr;
use crate::net::RecvFromFlags;
use crate::poll::POLL_WAIT_QUEUE;
use crate::prelude::*;
use crate::process::WaitQueue;
use crate::user_buffer::{UserBufReader, UserBufWriter, UserBuffer, UserBufferMut};
use kevlar_vfs::socket_types::{SockAddrUn, AF_UNIX, ShutdownHow};
use kevlar_vfs::stat::Stat;

// ── Constants ────────────────────────────────────────────────────────

const UNIX_STREAM_BUF_SIZE: usize = 16384;
const BACKLOG_MAX: usize = 128;

// ── Global listener registry ─────────────────────────────────────────
//
// Maps bound paths to listener sockets so that connect() can find them.

static UNIX_LISTENERS: SpinLock<VecDeque<(String, Arc<UnixListener>)>> =
    SpinLock::new(VecDeque::new());

fn register_listener(path: &str, listener: &Arc<UnixListener>) {
    UNIX_LISTENERS
        .lock()
        .push_back((String::from(path), listener.clone()));
}

fn unregister_listener(path: &str) {
    UNIX_LISTENERS.lock().retain(|(p, _)| p != path);
}

fn find_listener(path: &str) -> Option<Arc<UnixListener>> {
    UNIX_LISTENERS
        .lock()
        .iter()
        .find(|(p, _)| p == path)
        .map(|(_, l)| l.clone())
}

// ── Ancillary data (SCM_RIGHTS) ──────────────────────────────────────

/// File descriptors transferred via SCM_RIGHTS are stored as Arc<OpenedFile>
/// until the receiver calls recvmsg to install them in their fd table.
pub enum AncillaryData {
    Rights(Vec<Arc<OpenedFile>>),
}

// ── UnixStream — connected bidirectional byte stream ─────────────────

struct StreamInner {
    buf: RingBuffer<u8, UNIX_STREAM_BUF_SIZE>,
    ancillary: Option<VecDeque<AncillaryData>>,
    shut_wr: bool,
}

/// One end of a connected Unix stream pair.
///
/// `tx` is owned by this end (written to by write), `rx` is the peer's `tx`
/// (read from by read). This half-duplex pairing means that when the peer
/// writes, the data shows up in our `rx`, and vice versa.
pub struct UnixStream {
    /// Our write buffer — peer reads from this.
    tx: Arc<SpinLock<StreamInner>>,
    /// Peer's write buffer — we read from this.
    rx: Arc<SpinLock<StreamInner>>,
    /// Set when the peer's Arc is dropped (peer closed).
    peer_closed: Arc<AtomicBool>,
    /// Set when our side of the pair is the one that set peer_closed on drop.
    our_peer_flag: Arc<AtomicBool>,
    /// Bound path (for getsockname).
    bound_path: SpinLock<Option<String>>,
    /// Peer path (for getpeername).
    peer_path: SpinLock<Option<String>>,
    /// Socket type: SOCK_STREAM (1) or SOCK_DGRAM (2).
    sock_type: i32,
}

impl UnixStream {
    /// Create a connected pair of Unix sockets with the given type.
    pub fn new_pair_typed(sock_type: i32) -> (Arc<UnixStream>, Arc<UnixStream>) {
        let buf_a = Arc::new(SpinLock::new(StreamInner {
            buf: RingBuffer::new(),
            ancillary: None,
            shut_wr: false,
        }));
        let buf_b = Arc::new(SpinLock::new(StreamInner {
            buf: RingBuffer::new(),
            ancillary: None,
            shut_wr: false,
        }));

        let peer_flag = Arc::new(AtomicBool::new(false));

        let a = Arc::new(UnixStream {
            tx: buf_a.clone(),
            rx: buf_b.clone(),
            peer_closed: peer_flag.clone(),
            our_peer_flag: peer_flag.clone(),
            bound_path: SpinLock::new(None),
            peer_path: SpinLock::new(None),
            sock_type,
        });
        let b = Arc::new(UnixStream {
            tx: buf_b,
            rx: buf_a,
            peer_closed: peer_flag.clone(),
            our_peer_flag: peer_flag,
            bound_path: SpinLock::new(None),
            peer_path: SpinLock::new(None),
            sock_type,
        });

        (a, b)
    }

    /// Create a connected pair of Unix STREAM sockets (default).
    pub fn new_pair() -> (Arc<UnixStream>, Arc<UnixStream>) {
        Self::new_pair_typed(1) // SOCK_STREAM
    }

    /// Push ancillary data to be received by the peer.
    pub fn send_ancillary(&self, data: AncillaryData) {
        self.tx.lock().ancillary.get_or_insert_with(VecDeque::new).push_back(data);
    }

    /// Pop ancillary data from our receive side.
    pub fn recv_ancillary(&self) -> Option<AncillaryData> {
        self.rx.lock().ancillary.as_mut()?.pop_front()
    }

    /// Read one DGRAM message (2-byte LE length prefix + payload) from the ring buffer.
    fn dgram_read_one(
        buf: &mut RingBuffer<u8, UNIX_STREAM_BUF_SIZE>,
        writer: &mut UserBufWriter<'_>,
    ) -> Result<()> {
        // Peek at the 2-byte length header.
        let mut hdr = [0u8; 2];
        if let Some(b0) = buf.pop_slice(1) {
            hdr[0] = b0[0];
        } else {
            return Ok(()); // No data.
        }
        if let Some(b1) = buf.pop_slice(1) {
            hdr[1] = b1[0];
        } else {
            return Ok(()); // Incomplete header — shouldn't happen.
        }
        let msg_len = u16::from_le_bytes(hdr) as usize;

        // Read exactly msg_len bytes (or until user buffer full).
        let mut remaining = msg_len;
        while remaining > 0 {
            let to_read = core::cmp::min(remaining, writer.remaining_len());
            if to_read == 0 {
                // User buffer full — discard rest of message.
                while remaining > 0 {
                    if let Some(discard) = buf.pop_slice(remaining) {
                        remaining -= discard.len();
                    } else {
                        break;
                    }
                }
                break;
            }
            if let Some(src) = buf.pop_slice(to_read) {
                let n = src.len();
                writer.write_bytes(src)?;
                remaining -= n;
            } else {
                break;
            }
        }
        Ok(())
    }
}

impl Drop for UnixStream {
    fn drop(&mut self) {
        // Signal peer that we're closed.
        self.our_peer_flag.store(true, Ordering::Release);
        // Mark our tx as shut down so peer reads get EOF.
        self.tx.lock_no_irq().shut_wr = true;
        POLL_WAIT_QUEUE.wake_all();
    }
}

impl FileLike for UnixStream {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat::zeroed())
    }

    fn socket_type(&self) -> i32 {
        self.sock_type
    }

    fn is_seekable(&self) -> bool {
        false
    }

    fn read(
        &self,
        _offset: usize,
        buf: UserBufferMut<'_>,
        options: &OpenOptions,
    ) -> Result<usize> {
        let mut writer = UserBufWriter::from(buf);
        let is_dgram = self.sock_type == 2; // SOCK_DGRAM

        // Fast path.
        {
            let mut rx = self.rx.lock();
            if is_dgram {
                // DGRAM: read one length-prefixed message.
                Self::dgram_read_one(&mut rx.buf, &mut writer)?;
            } else {
                while let Some(src) = rx.buf.pop_slice(writer.remaining_len()) {
                    writer.write_bytes(src)?;
                }
            }

            if writer.written_len() > 0 {
                drop(rx);
                POLL_WAIT_QUEUE.wake_all();
                return Ok(writer.written_len());
            }

            // EOF: peer shut down write side and buffer is empty.
            if rx.shut_wr || self.peer_closed.load(Ordering::Acquire) {
                return Ok(0);
            }

            if options.nonblock {
                return Err(Errno::EAGAIN.into());
            }
        }

        // Slow path: block until data or EOF.
        let ret = POLL_WAIT_QUEUE.sleep_signalable_until(|| {
            let mut rx = self.rx.lock();

            if is_dgram {
                Self::dgram_read_one(&mut rx.buf, &mut writer)?;
            } else {
                while let Some(src) = rx.buf.pop_slice(writer.remaining_len()) {
                    writer.write_bytes(src)?;
                }
            }

            if writer.written_len() > 0 {
                Ok(Some(writer.written_len()))
            } else if rx.shut_wr || self.peer_closed.load(Ordering::Acquire) {
                Ok(Some(0)) // EOF
            } else {
                Ok(None)
            }
        });

        POLL_WAIT_QUEUE.wake_all();
        ret
    }

    fn write(
        &self,
        _offset: usize,
        buf: UserBuffer<'_>,
        options: &OpenOptions,
    ) -> Result<usize> {
        // Check if peer closed.
        if self.peer_closed.load(Ordering::Acquire) {
            return Err(Errno::EPIPE.into());
        }
        let mut reader = UserBufReader::from(buf);
        let is_dgram = self.sock_type == 2; // SOCK_DGRAM

        // Fast path.
        {
            let mut tx = self.tx.lock();
            if tx.shut_wr {
                return Err(Errno::EPIPE.into());
            }

            if is_dgram {
                // DGRAM: write 2-byte LE length prefix + entire message atomically.
                let msg_len = reader.remaining_len();
                let needed = 2 + msg_len;
                if tx.buf.free() >= needed {
                    let hdr = (msg_len as u16).to_le_bytes();
                    let dst = tx.buf.writable_contiguous();
                    if dst.len() >= 2 {
                        dst[..2].copy_from_slice(&hdr);
                        tx.buf.advance_write(2);
                    }
                    let mut written = 0;
                    while reader.remaining_len() > 0 {
                        let dst = tx.buf.writable_contiguous();
                        if dst.is_empty() { break; }
                        let copied = reader.read_bytes(dst)?;
                        if copied == 0 { break; }
                        tx.buf.advance_write(copied);
                        written += copied;
                    }
                    drop(tx);
                    POLL_WAIT_QUEUE.wake_all();
                    return Ok(written);
                }
                if options.nonblock {
                    return Err(Errno::EAGAIN.into());
                }
            } else {
                let mut written = 0;
                loop {
                    let dst = tx.buf.writable_contiguous();
                    if dst.is_empty() || reader.remaining_len() == 0 {
                        break;
                    }
                    let copied = reader.read_bytes(dst)?;
                    if copied == 0 {
                        break;
                    }
                    tx.buf.advance_write(copied);
                    written += copied;
                }

                if written > 0 {
                    drop(tx);
                    POLL_WAIT_QUEUE.wake_all();
                    return Ok(written);
                }

                if options.nonblock {
                    return Err(Errno::EAGAIN.into());
                }
            }
        }

        // Slow path.
        let ret = POLL_WAIT_QUEUE.sleep_signalable_until(|| {
            if self.peer_closed.load(Ordering::Acquire) {
                return Err(Errno::EPIPE.into());
            }

            let mut tx = self.tx.lock();
            if tx.shut_wr {
                return Err(Errno::EPIPE.into());
            }

            let mut written = 0;
            loop {
                let dst = tx.buf.writable_contiguous();
                if dst.is_empty() || reader.remaining_len() == 0 {
                    break;
                }
                let copied = reader.read_bytes(dst)?;
                if copied == 0 {
                    break;
                }
                tx.buf.advance_write(copied);
                written += copied;
            }

            if written > 0 {
                Ok(Some(written))
            } else {
                Ok(None)
            }
        });

        POLL_WAIT_QUEUE.wake_all();
        ret
    }

    fn sendto(
        &self,
        buf: UserBuffer<'_>,
        _sockaddr: Option<SockAddr>,
        options: &OpenOptions,
    ) -> Result<usize> {
        self.write(0, buf, options)
    }

    fn recvfrom(
        &self,
        buf: UserBufferMut<'_>,
        _flags: RecvFromFlags,
        options: &OpenOptions,
    ) -> Result<(usize, SockAddr)> {
        let len = self.read(0, buf, options)?;
        // Return a blank AF_UNIX sockaddr.
        Ok((len, SockAddr::Un(SockAddrUn {
            family: AF_UNIX as u16,
            path: [0u8; 108],
        })))
    }

    fn shutdown(&self, how: ShutdownHow) -> Result<()> {
        match how {
            ShutdownHow::Wr | ShutdownHow::RdWr => {
                self.tx.lock().shut_wr = true;
            }
            ShutdownHow::Rd => {
                // Mark peer's tx as shut so they get EPIPE.
                self.rx.lock().shut_wr = true;
            }
        }
        POLL_WAIT_QUEUE.wake_all();
        Ok(())
    }

    fn getsockname(&self) -> Result<SockAddr> {
        let path_opt = self.bound_path.lock().clone();
        let mut sa = SockAddrUn {
            family: AF_UNIX as u16,
            path: [0u8; 108],
        };
        if let Some(p) = path_opt {
            let bytes = p.as_bytes();
            let len = core::cmp::min(bytes.len(), 107);
            sa.path[..len].copy_from_slice(&bytes[..len]);
        }
        Ok(SockAddr::Un(sa))
    }

    fn getpeername(&self) -> Result<SockAddr> {
        let path_opt = self.peer_path.lock().clone();
        let mut sa = SockAddrUn {
            family: AF_UNIX as u16,
            path: [0u8; 108],
        };
        if let Some(p) = path_opt {
            let bytes = p.as_bytes();
            let len = core::cmp::min(bytes.len(), 107);
            sa.path[..len].copy_from_slice(&bytes[..len]);
        }
        Ok(SockAddr::Un(sa))
    }

    fn poll(&self) -> Result<PollStatus> {
        let mut status = PollStatus::empty();

        {
            let rx = self.rx.lock();
            if rx.buf.is_readable() {
                status |= PollStatus::POLLIN;
            }
            if rx.shut_wr || self.peer_closed.load(Ordering::Acquire) {
                // Peer closed — signal POLLIN for EOF + POLLHUP.
                status |= PollStatus::POLLIN | PollStatus::POLLHUP;
            }
        }

        {
            let tx = self.tx.lock();
            if tx.buf.is_writable() && !tx.shut_wr {
                status |= PollStatus::POLLOUT;
            }
        }

        Ok(status)
    }
}

impl fmt::Debug for UnixStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnixStream").finish()
    }
}

// ── UnixListener — bound, listening socket ───────────────────────────

struct ListenerInner {
    backlog: VecDeque<Arc<UnixStream>>,
    max_backlog: usize,
}

pub struct UnixListener {
    path: SpinLock<Option<String>>,
    inner: SpinLock<ListenerInner>,
    wait_queue: WaitQueue,
}

impl UnixListener {
    fn new() -> Arc<UnixListener> {
        Arc::new(UnixListener {
            path: SpinLock::new(None),
            inner: SpinLock::new(ListenerInner {
                backlog: VecDeque::new(),
                max_backlog: BACKLOG_MAX,
            }),
            wait_queue: WaitQueue::new(),
        })
    }

    /// Called by connect(): push the server's end of a new stream into the
    /// backlog. Returns the client's end.
    fn enqueue_connection(&self, client_path: Option<&str>) -> Result<Arc<UnixStream>> {
        let (server_end, client_end) = UnixStream::new_pair();

        // Set path metadata.
        if let Some(p) = self.path.lock().as_deref() {
            *client_end.peer_path.lock() = Some(String::from(p));
            *server_end.bound_path.lock() = Some(String::from(p));
        }
        if let Some(cp) = client_path {
            *server_end.peer_path.lock() = Some(String::from(cp));
            *client_end.bound_path.lock() = Some(String::from(cp));
        }

        let mut inner = self.inner.lock();
        if inner.backlog.len() >= inner.max_backlog {
            return Err(Errno::ECONNREFUSED.into());
        }
        inner.backlog.push_back(server_end);
        drop(inner);

        self.wait_queue.wake_all();
        POLL_WAIT_QUEUE.wake_all();

        Ok(client_end)
    }
}

impl FileLike for UnixListener {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat::zeroed())
    }

    fn accept(&self, options: &OpenOptions) -> Result<(Arc<dyn FileLike>, SockAddr)> {
        // Fast path.
        {
            let mut inner = self.inner.lock();
            if let Some(stream) = inner.backlog.pop_front() {
                let sa = stream.getsockname().unwrap_or_else(|_| SockAddr::Un(SockAddrUn {
                    family: AF_UNIX as u16,
                    path: [0u8; 108],
                }));
                return Ok((stream as Arc<dyn FileLike>, sa));
            }

            if options.nonblock {
                return Err(Errno::EAGAIN.into());
            }
        }

        // Slow path: wait for incoming connection.
        self.wait_queue.sleep_signalable_until(|| {
            let mut inner = self.inner.lock();
            if let Some(stream) = inner.backlog.pop_front() {
                let sa = stream.getsockname().unwrap_or_else(|_| SockAddr::Un(SockAddrUn {
                    family: AF_UNIX as u16,
                    path: [0u8; 108],
                }));
                Ok(Some((stream as Arc<dyn FileLike>, sa)))
            } else {
                Ok(None)
            }
        })
    }

    fn getsockname(&self) -> Result<SockAddr> {
        let path_opt = self.path.lock().clone();
        let mut sa = SockAddrUn {
            family: AF_UNIX as u16,
            path: [0u8; 108],
        };
        if let Some(p) = path_opt {
            let bytes = p.as_bytes();
            let len = core::cmp::min(bytes.len(), 107);
            sa.path[..len].copy_from_slice(&bytes[..len]);
        }
        Ok(SockAddr::Un(sa))
    }

    fn poll(&self) -> Result<PollStatus> {
        let inner = self.inner.lock();
        let mut status = PollStatus::empty();
        if !inner.backlog.is_empty() {
            status |= PollStatus::POLLIN;
        }
        Ok(status)
    }
}

impl Drop for UnixListener {
    fn drop(&mut self) {
        if let Some(path) = self.path.lock().as_deref() {
            unregister_listener(path);
        }
    }
}

impl fmt::Debug for UnixListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnixListener").finish()
    }
}

// ── UnixSocket — the initial socket() object (state machine) ─────────
//
// States: Created → (bind → Bound → listen → Listening) or (connect → Connected)
//
// After listen(), the socket becomes an UnixListener (backlog-based accept).
// After connect(), the socket becomes an UnixStream (bidirectional I/O).
// We implement this as an enum inside a SpinLock so that one Arc<UnixSocket>
// can transition between states.

enum SocketState {
    /// Just created with socket(AF_UNIX, SOCK_STREAM, 0).
    Created,
    /// bind() was called — path is set but not listening yet.
    Bound(String),
    /// listen() was called — delegate to UnixListener.
    Listening(Arc<UnixListener>),
    /// connect() was called — delegate to UnixStream.
    Connected(Arc<UnixStream>),
}

pub struct UnixSocket {
    state: SpinLock<SocketState>,
    sock_type: i32,
}

impl UnixSocket {
    pub fn new() -> Arc<UnixSocket> {
        Arc::new(UnixSocket {
            state: SpinLock::new(SocketState::Created),
            sock_type: 1, // SOCK_STREAM default
        })
    }

    pub fn new_typed(sock_type: i32) -> Arc<UnixSocket> {
        Arc::new(UnixSocket {
            state: SpinLock::new(SocketState::Created),
            sock_type,
        })
    }

    /// Get the inner UnixStream if this socket is in the Connected state.
    /// Used by sendmsg/recvmsg for ancillary data (SCM_RIGHTS).
    pub fn connected_stream(&self) -> Option<Arc<UnixStream>> {
        let state = self.state.lock();
        match &*state {
            SocketState::Connected(s) => Some(s.clone()),
            _ => None,
        }
    }
}

/// Extract a NUL-terminated path from a SockAddrUn.
fn sockaddr_un_path(sa: &SockAddrUn) -> Result<&str> {
    // Find NUL terminator in the path field.
    let nul_pos = sa.path.iter().position(|&b| b == 0).unwrap_or(sa.path.len());
    if nul_pos == 0 {
        return Err(Errno::EINVAL.into());
    }
    core::str::from_utf8(&sa.path[..nul_pos]).map_err(|_| Error::new(Errno::EINVAL))
}

impl FileLike for UnixSocket {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat::zeroed())
    }

    fn socket_type(&self) -> i32 {
        self.sock_type
    }

    fn is_seekable(&self) -> bool {
        false
    }

    fn bind(&self, sockaddr: SockAddr) -> Result<()> {
        let sa = match &sockaddr {
            SockAddr::Un(sa) => sa,
            _ => return Err(Errno::EINVAL.into()),
        };

        let path = sockaddr_un_path(sa)?;

        let mut state = self.state.lock();
        match &*state {
            SocketState::Created => {
                *state = SocketState::Bound(String::from(path));
                Ok(())
            }
            _ => Err(Errno::EINVAL.into()),
        }
    }

    fn listen(&self, backlog: i32) -> Result<()> {
        let mut state = self.state.lock();
        let path = match &*state {
            SocketState::Bound(path) => path.clone(),
            _ => return Err(Errno::EINVAL.into()),
        };

        let listener = UnixListener::new();
        *listener.path.lock() = Some(path.clone());
        {
            let mut inner = listener.inner.lock();
            inner.max_backlog = core::cmp::min(backlog.max(1) as usize, BACKLOG_MAX);
        }

        register_listener(&path, &listener);
        *state = SocketState::Listening(listener);
        Ok(())
    }

    fn accept(&self, options: &OpenOptions) -> Result<(Arc<dyn FileLike>, SockAddr)> {
        let listener = {
            let state = self.state.lock();
            match &*state {
                SocketState::Listening(l) => l.clone(),
                _ => return Err(Errno::EINVAL.into()),
            }
        };

        listener.accept(options)
    }

    fn connect(&self, sockaddr: SockAddr, _options: &OpenOptions) -> Result<()> {
        let sa = match &sockaddr {
            SockAddr::Un(sa) => sa,
            _ => return Err(Errno::EINVAL.into()),
        };

        let path = sockaddr_un_path(sa)?;

        // For DGRAM sockets, connect just sets the default destination.
        // If there's no listener, accept silently (systemd's sd_notify pattern).
        let listener = match find_listener(path) {
            Some(l) => l,
            None => {
                // No listener — for DGRAM sockets this is fine.
                // Just store the peer address and return success.
                let state = self.state.lock();
                match &*state {
                    SocketState::Created | SocketState::Bound(_) => {
                        // Mark as "connected" by storing peer path.
                        // Actual sends will be no-ops until a receiver binds.
                    }
                    _ => {}
                }
                return Ok(());
            }
        };
        let client_end = listener.enqueue_connection(None)?;

        let mut state = self.state.lock();
        *state = SocketState::Connected(client_end);
        Ok(())
    }

    fn read(
        &self,
        offset: usize,
        buf: UserBufferMut<'_>,
        options: &OpenOptions,
    ) -> Result<usize> {
        let stream = {
            let state = self.state.lock();
            match &*state {
                SocketState::Connected(s) => s.clone(),
                _ => return Err(Errno::ENOTCONN.into()),
            }
        };
        stream.read(offset, buf, options)
    }

    fn write(
        &self,
        offset: usize,
        buf: UserBuffer<'_>,
        options: &OpenOptions,
    ) -> Result<usize> {
        let stream = {
            let state = self.state.lock();
            match &*state {
                SocketState::Connected(s) => s.clone(),
                _ => return Err(Errno::ENOTCONN.into()),
            }
        };
        stream.write(offset, buf, options)
    }

    fn sendto(
        &self,
        buf: UserBuffer<'_>,
        sockaddr: Option<SockAddr>,
        options: &OpenOptions,
    ) -> Result<usize> {
        let stream = {
            let state = self.state.lock();
            match &*state {
                SocketState::Connected(s) => s.clone(),
                _ => return Err(Errno::ENOTCONN.into()),
            }
        };
        stream.sendto(buf, sockaddr, options)
    }

    fn recvfrom(
        &self,
        buf: UserBufferMut<'_>,
        flags: RecvFromFlags,
        options: &OpenOptions,
    ) -> Result<(usize, SockAddr)> {
        let stream = {
            let state = self.state.lock();
            match &*state {
                SocketState::Connected(s) => s.clone(),
                _ => return Err(Errno::ENOTCONN.into()),
            }
        };
        stream.recvfrom(buf, flags, options)
    }

    fn shutdown(&self, how: ShutdownHow) -> Result<()> {
        let stream = {
            let state = self.state.lock();
            match &*state {
                SocketState::Connected(s) => s.clone(),
                _ => return Err(Errno::ENOTCONN.into()),
            }
        };
        stream.shutdown(how)
    }

    fn getsockname(&self) -> Result<SockAddr> {
        let state = self.state.lock();
        match &*state {
            SocketState::Bound(path) => {
                let mut sa = SockAddrUn {
                    family: AF_UNIX as u16,
                    path: [0u8; 108],
                };
                let bytes = path.as_bytes();
                let len = core::cmp::min(bytes.len(), 107);
                sa.path[..len].copy_from_slice(&bytes[..len]);
                Ok(SockAddr::Un(sa))
            }
            SocketState::Listening(l) => l.getsockname(),
            SocketState::Connected(s) => s.getsockname(),
            _ => Ok(SockAddr::Un(SockAddrUn {
                family: AF_UNIX as u16,
                path: [0u8; 108],
            })),
        }
    }

    fn getpeername(&self) -> Result<SockAddr> {
        let state = self.state.lock();
        match &*state {
            SocketState::Connected(s) => s.getpeername(),
            _ => Err(Errno::ENOTCONN.into()),
        }
    }

    fn poll(&self) -> Result<PollStatus> {
        let state = self.state.lock();
        match &*state {
            SocketState::Listening(l) => l.poll(),
            SocketState::Connected(s) => s.poll(),
            _ => Ok(PollStatus::POLLOUT), // unconnected socket is writable
        }
    }
}

impl fmt::Debug for UnixSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnixSocket").finish()
    }
}
