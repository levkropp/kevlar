// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! AF_UNIX (Unix domain) socket implementation.
//!
//! Supports SOCK_STREAM named sockets: bind to a path, listen, accept, connect,
//! bidirectional byte-stream I/O with poll support, and SCM_RIGHTS fd passing
//! via sendmsg/recvmsg ancillary data.
//!
//! Provenance: Own (Linux unix(7), cmsg(3) man pages).
use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::{Arc, Weak};

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
use kevlar_vfs::inode::INodeNo;
use kevlar_vfs::socket_types::{SockAddrUn, AF_UNIX, ShutdownHow};
use kevlar_vfs::stat::{FileMode, Stat, S_IFSOCK};

// ── Constants ────────────────────────────────────────────────────────

// Per-direction AF_UNIX SOCK_STREAM buffer.  Was 16 KiB; Linux's
// default is ~200 KiB.  An X server holding SubstructureNotify on
// root delivers events to subscribing clients, and the client's
// recv buffer fills with backlog events when the client is busy
// in initialisation — at 16 KiB that backpressure-deadlocks the
// server within a few seconds of a typical openbox startup
// (verified blog 239 via kxproxy: ~12.7 KiB of S2C bytes captured
// before openbox stops responding, which is consistent with the
// 16 KiB recv buffer hitting full with the next event).
//
// 256 KiB matches Linux's effective default for typical workloads
// and is comfortably above what openbox's startup needs.
const UNIX_STREAM_BUF_SIZE: usize = 262144;
const BACKLOG_MAX: usize = 128;

/// Per-socket inode counter so `/proc/<pid>/fd/N → socket:[INODE]`
/// has stable, distinguishable values across instances (matching
/// Linux's behaviour, which `lsof` and friends key off).
static SOCKET_INODE_COUNTER: AtomicU64 = AtomicU64::new(1);

fn alloc_socket_inode() -> u64 {
    SOCKET_INODE_COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn socket_stat(inode: u64) -> Stat {
    let mut st = Stat::zeroed();
    st.inode_no = INodeNo::new(inode as usize);
    st.mode = FileMode::new(S_IFSOCK | 0o600);
    st
}

// ── Global listener registry ─────────────────────────────────────────
//
// Maps bound listener identities to live `UnixListener` instances so
// connect() can route a new connection to the right server.
//
// **Identity is keyed differently for the two AF_UNIX namespaces:**
//
// * Abstract sockets (sun_path[0] == NUL) live entirely in-memory and
//   have no filesystem presence on Linux either.  They are keyed by
//   their `@name` string, since that's the only stable handle.
//
// * Filesystem sockets are keyed by `(dev_id, inode_no)` — the on-disk
//   identity of the bound socket file.  This matches Linux: the inode
//   is the listener's identity, not the path.  Consequences:
//
//     - Renaming the socket file (mv) keeps connections routing correctly,
//       because the inode survives the rename.
//     - Unlinking the file makes connect() return ENOENT (path lookup
//       fails) — the listener is still alive, but unreachable by path.
//     - A second bind() to a stale socket path returns EADDRINUSE,
//       again because the path lookup at bind sees an existing entry.
//
// The registry holds `Weak<UnixListener>` (NOT strong Arc).  A strong Arc
// would keep the listener alive forever even after the owning fd was
// closed and the owning process exited, and `UnixListener::drop` (which
// removes the entry) would never run.  With `Weak`, when the last strong
// reference (the owning fd) is dropped, `UnixListener::drop` fires and
// removes the entry.  `find_listener` skips any dangling `Weak`s it
// finds and opportunistically prunes them.

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ListenerKey {
    /// Abstract namespace: the leading `@` is part of the stored string
    /// (matches `getsockname`-displayed form).
    Abstract(String),
    /// Filesystem-namespace socket.  `(dev_id, inode_no)` is the on-disk
    /// identity, but on disk-backed filesystems (ext2/4) inode numbers
    /// are recycled after `unlink + free`.  A long-lived listener whose
    /// file was unlinked could collide with a *different* file later
    /// bound at the same inode number, routing connects to the wrong
    /// listener.  The `generation` field disambiguates: it's bumped on
    /// every successful `bind()` to a given `(dev, inode_no)` and held
    /// in `SOCKET_GENERATIONS` until the matching listener drops.
    /// Tmpfs uses monotonic inode numbers and never recycles, so the
    /// generation just stays at 1 there — no harm done.
    Inode { dev_id: usize, inode_no: u64, generation: u32 },
}

/// Per-(dev, inode) generation counter.  See `ListenerKey::Inode` for why.
///
/// Lifecycle:
/// * `bind()` calls `bump_socket_generation` to mint a fresh value.
/// * `connect()` calls `current_socket_generation` to read whichever
///   generation is currently bound at this inode (or `None` if no socket
///   is bound there — yields ECONNREFUSED).
/// * `UnixListener::drop` calls `release_socket_generation`, which only
///   removes the entry if the dropping listener's generation is still
///   the current one (otherwise a newer bind has already taken over).
static SOCKET_GENERATIONS: SpinLock<BTreeMap<(usize, u64), u32>> =
    SpinLock::new(BTreeMap::new());

fn bump_socket_generation(dev_id: usize, inode_no: u64) -> u32 {
    let mut gens = SOCKET_GENERATIONS.lock();
    let entry = gens.entry((dev_id, inode_no)).or_insert(0);
    *entry = entry.saturating_add(1);
    *entry
}

fn current_socket_generation(dev_id: usize, inode_no: u64) -> Option<u32> {
    SOCKET_GENERATIONS.lock().get(&(dev_id, inode_no)).copied()
}

fn release_socket_generation(dev_id: usize, inode_no: u64, generation: u32) {
    let mut gens = SOCKET_GENERATIONS.lock();
    if gens.get(&(dev_id, inode_no)).copied() == Some(generation) {
        gens.remove(&(dev_id, inode_no));
    }
}

static UNIX_LISTENERS: SpinLock<VecDeque<(ListenerKey, Weak<UnixListener>)>> =
    SpinLock::new(VecDeque::new());

fn register_listener(key: ListenerKey, listener: &Arc<UnixListener>) {
    let mut table = UNIX_LISTENERS.lock();
    // Purge any existing entries for this key — dead ones (dangling Weak)
    // and live ones (should be rare; bind() now returns EADDRINUSE on a
    // double-bind via filesystem-path collision, but abstract sockets can
    // still race here, in which case we let the newest listener win).
    table.retain(|(k, _)| k != &key);
    table.push_back((key, Arc::downgrade(listener)));
}

fn unregister_listener(key: &ListenerKey) {
    UNIX_LISTENERS.lock().retain(|(k, _)| k != key);
}

fn find_listener(key: &ListenerKey) -> Option<Arc<UnixListener>> {
    let mut table = UNIX_LISTENERS.lock();
    // Scan for a matching, still-live listener.  Opportunistically prune
    // dangling Weak entries while we iterate.
    let mut found: Option<Arc<UnixListener>> = None;
    table.retain(|(k, w)| {
        match w.upgrade() {
            Some(arc) => {
                if found.is_none() && k == key {
                    found = Some(arc);
                }
                true
            }
            None => false, // dangling — remove
        }
    });
    found
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

/// One direction of a Unix stream pair.  Wraps the buffer behind a
/// SpinLock and adds two lock-free atomics for EPOLLET edge tracking
/// (kept outside the lock so `poll_gen` doesn't have to acquire it
/// on every epoll_wait iteration).  Pattern mirrors
/// `kernel/pipe.rs::PipeInner`.
struct StreamSide {
    inner: SpinLock<Box<StreamInner>>,
    /// Bumped on every state change (write success, read drain,
    /// peer shutdown).  `UnixStream::poll_gen` returns the SUM of
    /// `tx.state_gen + rx.state_gen` so a change on either direction
    /// is observable.  Initial value 1 so any bump is distinct from
    /// the zero-initialized `last_gen` cached in the epoll Interest.
    state_gen: AtomicU64,
    /// EPOLLET watcher count.  Bumps to `state_gen` are gated on
    /// this >0 — saves an atomic RMW per syscall in the common
    /// no-ET case.
    et_watcher_count: AtomicU32,
}

impl StreamSide {
    /// Bump state_gen with Release ordering, gated on the et watcher
    /// count being non-zero.  Call AFTER the buffer state change but
    /// BEFORE the wake_all so any thread woken sees the new gen.
    #[inline]
    fn bump_gen(&self) {
        if self.et_watcher_count.load(Ordering::Relaxed) > 0 {
            self.state_gen.fetch_add(1, Ordering::Release);
        }
    }
}

/// One end of a connected Unix stream pair.
///
/// `tx` is owned by this end (written to by write), `rx` is the peer's `tx`
/// (read from by read). This half-duplex pairing means that when the peer
/// writes, the data shows up in our `rx`, and vice versa.
pub struct UnixStream {
    /// Our write buffer — peer reads from this.
    /// Box-allocated because StreamInner is 16KB+ (the ring buffer).
    /// Can't live on the 8KB syscall_stack.
    tx: Arc<StreamSide>,
    /// Peer's write buffer — we read from this.
    rx: Arc<StreamSide>,
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
    /// Peer credentials captured at connection time (for SO_PEERCRED).
    /// On Linux, SO_PEERCRED returns the peer's pid/uid/gid at the time
    /// of connect()/socketpair(), NOT the current process's credentials.
    peer_pid: core::sync::atomic::AtomicI32,
    peer_uid: core::sync::atomic::AtomicU32,
    peer_gid: core::sync::atomic::AtomicU32,
    /// Pseudo-inode for `/proc/<pid>/fd/N → socket:[INODE]`.
    inode_no: u64,
}

/// Allocate a StreamInner directly on the heap.
///
/// StreamInner contains a 16KB RingBuffer<u8, 16384> inline array.
/// Box::new() / Arc::new(SpinLock::new(StreamInner { .. })) would construct
/// it on the stack first, overflowing the 8KB syscall_stack and corrupting
/// adjacent physical memory (same class of bug as the pipe stack overflow
/// fixed in e5366c0).
///
/// The 16 KB ring-buffer data area is MaybeUninit<u8> and doesn't need
/// zeroing — `rp == wp == 0` and `full == false` mean "no bytes valid".
/// Zeroing 16 KB on every socketpair was ~1.6 µs of wasted bandwidth; the
/// in-place write of just the metadata fields is ~20 ns.
#[allow(unsafe_code)]
fn alloc_stream_inner() -> Box<StreamInner> {
    unsafe {
        let layout = core::alloc::Layout::new::<StreamInner>();
        let ptr = alloc::alloc::alloc(layout) as *mut StreamInner;
        assert!(!ptr.is_null(), "unix stream: failed to allocate StreamInner");
        // Initialize the metadata fields in place; leave the 16 KB
        // MaybeUninit<u8> ring-buffer data uninitialized.
        core::ptr::addr_of_mut!((*ptr).buf).write(RingBuffer::new());
        core::ptr::addr_of_mut!((*ptr).ancillary).write(None);
        core::ptr::addr_of_mut!((*ptr).shut_wr).write(false);
        Box::from_raw(ptr)
    }
}

/// Wrap an in-place-allocated StreamInner in a SpinLock + atomics.
fn alloc_stream_side() -> Arc<StreamSide> {
    Arc::new(StreamSide {
        inner: SpinLock::new(alloc_stream_inner()),
        state_gen: AtomicU64::new(1),
        et_watcher_count: AtomicU32::new(0),
    })
}

impl UnixStream {
    /// Create a connected pair of Unix sockets with the given type.
    pub fn new_pair_typed(sock_type: i32) -> (Arc<UnixStream>, Arc<UnixStream>) {
        let buf_a = alloc_stream_side();
        let buf_b = alloc_stream_side();

        let peer_flag = Arc::new(AtomicBool::new(false));

        // Capture current process credentials for both sides of the pair.
        // For socketpair(): both sides belong to the current process.
        // For accept(): the server side's peer is the connecting client
        // (set later in enqueue_connection).
        let proc = crate::process::current_process();
        let cur_pid = proc.pid().as_i32();
        let cur_uid = proc.uid();
        let cur_gid = proc.gid();

        let a = Arc::new(UnixStream {
            tx: buf_a.clone(),
            rx: buf_b.clone(),
            peer_closed: peer_flag.clone(),
            our_peer_flag: peer_flag.clone(),
            bound_path: SpinLock::new(None),
            peer_path: SpinLock::new(None),
            sock_type,
            peer_pid: core::sync::atomic::AtomicI32::new(cur_pid),
            peer_uid: core::sync::atomic::AtomicU32::new(cur_uid),
            peer_gid: core::sync::atomic::AtomicU32::new(cur_gid),
            inode_no: alloc_socket_inode(),
        });
        let b = Arc::new(UnixStream {
            tx: buf_b,
            rx: buf_a,
            peer_closed: peer_flag.clone(),
            our_peer_flag: peer_flag,
            bound_path: SpinLock::new(None),
            peer_path: SpinLock::new(None),
            sock_type,
            peer_pid: core::sync::atomic::AtomicI32::new(cur_pid),
            peer_uid: core::sync::atomic::AtomicU32::new(cur_uid),
            peer_gid: core::sync::atomic::AtomicU32::new(cur_gid),
            inode_no: alloc_socket_inode(),
        });

        (a, b)
    }

    /// Create a connected pair of Unix STREAM sockets (default).
    pub fn new_pair() -> (Arc<UnixStream>, Arc<UnixStream>) {
        Self::new_pair_typed(1) // SOCK_STREAM
    }

    /// Returns the peer's credentials (pid, uid, gid) captured at connection time.
    /// Used by SO_PEERCRED getsockopt.
    pub fn peer_cred(&self) -> (i32, u32, u32) {
        (
            self.peer_pid.load(core::sync::atomic::Ordering::Relaxed),
            self.peer_uid.load(core::sync::atomic::Ordering::Relaxed),
            self.peer_gid.load(core::sync::atomic::Ordering::Relaxed),
        )
    }

    /// Push ancillary data to be received by the peer.
    pub fn send_ancillary(&self, data: AncillaryData) {
        self.tx.inner.lock().ancillary.get_or_insert_with(VecDeque::new).push_back(data);
    }

    /// Pop ancillary data from our receive side.
    pub fn recv_ancillary(&self) -> Option<AncillaryData> {
        self.rx.inner.lock().ancillary.as_mut()?.pop_front()
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
        self.tx.inner.lock_no_irq().shut_wr = true;
        // Both our tx (peer-readable EOF) and our rx (peer's tx — we
        // can't read from it anymore) state changed; bump both so
        // EPOLLET watchers on either side see the close edge.
        self.tx.bump_gen();
        self.rx.bump_gen();
        POLL_WAIT_QUEUE.wake_all();
    }
}

impl FileLike for UnixStream {
    fn stat(&self) -> Result<Stat> {
        Ok(socket_stat(self.inode_no))
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
            let mut rx = self.rx.inner.lock();
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
                // Reading drained bytes from rx — peer's POLLOUT and
                // our POLLIN may have changed.  Bump rx side's gen
                // (peer's tx-from-our-perspective).  See StreamSide
                // doc for the EPOLLET edge contract.
                self.rx.bump_gen();
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
            let mut rx = self.rx.inner.lock();

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

        if ret.as_ref().map(|n| *n > 0).unwrap_or(false) {
            self.rx.bump_gen();
        }
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
            let mut tx = self.tx.inner.lock();
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
                    // We wrote bytes into our tx (peer's rx) — peer's
                    // POLLIN edge fires.  Bump tx side's gen.
                    self.tx.bump_gen();
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
                    // STREAM write — peer's POLLIN may have edged.
                    self.tx.bump_gen();
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

            let mut tx = self.tx.inner.lock();
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

        if ret.as_ref().map(|n| *n > 0).unwrap_or(false) {
            self.tx.bump_gen();
        }
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
                self.tx.inner.lock().shut_wr = true;
                // Peer reading from our tx now gets EOF — POLLIN edge.
                self.tx.bump_gen();
                if matches!(how, ShutdownHow::RdWr) {
                    self.rx.inner.lock().shut_wr = true;
                    self.rx.bump_gen();
                }
            }
            ShutdownHow::Rd => {
                // Mark peer's tx as shut so they get EPIPE.
                self.rx.inner.lock().shut_wr = true;
                self.rx.bump_gen();
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
            if let Some(name) = p.strip_prefix('@') {
                // Abstract socket: write NUL prefix + name
                sa.path[0] = 0;
                let bytes = name.as_bytes();
                let len = core::cmp::min(bytes.len(), 106);
                sa.path[1..1 + len].copy_from_slice(&bytes[..len]);
            } else {
                let bytes = p.as_bytes();
                let len = core::cmp::min(bytes.len(), 107);
                sa.path[..len].copy_from_slice(&bytes[..len]);
            }
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
            if let Some(name) = p.strip_prefix('@') {
                // Abstract socket: write NUL prefix + name
                sa.path[0] = 0;
                let bytes = name.as_bytes();
                let len = core::cmp::min(bytes.len(), 106);
                sa.path[1..1 + len].copy_from_slice(&bytes[..len]);
            } else {
                let bytes = p.as_bytes();
                let len = core::cmp::min(bytes.len(), 107);
                sa.path[..len].copy_from_slice(&bytes[..len]);
            }
        }
        Ok(SockAddr::Un(sa))
    }

    fn poll(&self) -> Result<PollStatus> {
        let mut status = PollStatus::empty();

        {
            let rx = self.rx.inner.lock();
            if rx.buf.is_readable() {
                status |= PollStatus::POLLIN;
            }
            if rx.shut_wr || self.peer_closed.load(Ordering::Acquire) {
                // Peer closed — signal POLLIN for EOF + POLLHUP.
                status |= PollStatus::POLLIN | PollStatus::POLLHUP;
            }
        }

        {
            let tx = self.tx.inner.lock();
            if tx.buf.is_writable() && !tx.shut_wr {
                status |= PollStatus::POLLOUT;
            }
        }

        Ok(status)
    }

    fn poll_gen(&self) -> u64 {
        // CRITICAL: return 0 when no ET watchers — the epoll
        // poll_cached path treats non-zero gen as "cache OK" and
        // serves stale poll status until gen changes.  Since we
        // only bump state_gen when et_count > 0, returning non-zero
        // here without a watcher would freeze the cache permanently.
        // Mirror `kernel/pipe.rs::poll_gen` exactly.
        if self.tx.et_watcher_count.load(Ordering::Relaxed) > 0
            || self.rx.et_watcher_count.load(Ordering::Relaxed) > 0
        {
            self.tx.state_gen.load(Ordering::Acquire)
                .wrapping_add(self.rx.state_gen.load(Ordering::Acquire))
        } else {
            0
        }
    }
    fn notify_epoll_et(&self, added: bool) {
        if added {
            self.tx.et_watcher_count.fetch_add(1, Ordering::Relaxed);
            self.rx.et_watcher_count.fetch_add(1, Ordering::Relaxed);
        } else {
            self.tx.et_watcher_count.fetch_sub(1, Ordering::Relaxed);
            self.rx.et_watcher_count.fetch_sub(1, Ordering::Relaxed);
        }
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
    /// Human-readable path (with leading `@` for abstract namespace) —
    /// returned by `getsockname()` and shown in `/proc/net/unix`.  This
    /// is *display only*; routing uses `key` below.
    path: SpinLock<Option<String>>,
    /// Registry key — the actual identity used by `find_listener`.
    /// Filesystem sockets carry `(dev_id, inode_no)`; abstract sockets
    /// carry the `@name` string.
    key: SpinLock<Option<ListenerKey>>,
    inner: SpinLock<ListenerInner>,
    wait_queue: WaitQueue,
    /// EPOLLET state-change generation.  Bumped when the backlog
    /// transitions (push on connect, pop on accept).  Lives outside
    /// the inner spinlock so `poll_gen()` can read it without
    /// taking the lock.  Initial value 1 so any bump is observably
    /// distinct from zero-initialized cached gen.
    state_gen: AtomicU64,
    /// Number of EPOLLET watchers on this listener fd.  Bumps to
    /// `state_gen` are gated on this >0.  Mirrors
    /// `kernel/pipe.rs::PipeInner::et_watcher_count`.
    et_watcher_count: AtomicU32,
    /// Pseudo-inode for `/proc/<pid>/fd/N → socket:[INODE]`.
    inode_no: u64,
}

impl UnixListener {
    fn new() -> Arc<UnixListener> {
        Arc::new(UnixListener {
            path: SpinLock::new(None),
            key: SpinLock::new(None),
            inner: SpinLock::new(ListenerInner {
                backlog: VecDeque::new(),
                max_backlog: BACKLOG_MAX,
            }),
            state_gen: AtomicU64::new(1),
            et_watcher_count: AtomicU32::new(0),
            wait_queue: WaitQueue::new(),
            inode_no: alloc_socket_inode(),
        })
    }

    /// Called by connect(): push the server's end of a new stream into the
    /// backlog. Returns the client's end.
    fn enqueue_connection(&self, client_path: Option<&str>) -> Result<Arc<UnixStream>> {
        let (server_end, client_end) = UnixStream::new_pair();

        // Capture credentials: the CLIENT_END's peer is the SERVER (listener),
        // and the SERVER_END's peer is the CLIENT (connector).
        // new_pair() sets both sides to current process. For connect(), the
        // client end's peer should be the listener process (unknown at this
        // point — set when accept() returns the server end). The server end's
        // peer should be the connecting process (current).
        // For SO_PEERCRED to work correctly:
        //   - On the client fd: peer = server (set when accept runs)
        //   - On the server fd: peer = client (set now from current_process)
        // For simplicity, set both to current_process now. The accept() side
        // will update when the server calls accept().
        let proc = crate::process::current_process();
        let client_pid = proc.pid().as_i32();
        let client_uid = proc.uid();
        let client_gid = proc.gid();
        // Server end's peer = connecting client
        server_end.peer_pid.store(client_pid, core::sync::atomic::Ordering::Relaxed);
        server_end.peer_uid.store(client_uid, core::sync::atomic::Ordering::Relaxed);
        server_end.peer_gid.store(client_gid, core::sync::atomic::Ordering::Relaxed);

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
        let bl_after = inner.backlog.len();
        drop(inner);

        // Backlog grew — listener's POLLIN edges.  Bump BEFORE the
        // wakes so any thread woken on POLL_WAIT_QUEUE re-checks
        // poll_gen and sees the new value.
        if self.et_watcher_count.load(Ordering::Relaxed) > 0 {
            self.state_gen.fetch_add(1, Ordering::Release);
        }

        // Diagnostic: gated on epoll-trace-fd cmdline being set.
        if crate::fs::epoll::EPOLL_TRACE_FD.load(Ordering::Relaxed) != 0 {
            let pq = POLL_WAIT_QUEUE.waiter_count();
            info!(
                "AF_UNIX enqueue: listener_inode={} bl={} pid={} POLL_WQ.waiters={}",
                self.inode_no, bl_after, client_pid, pq,
            );
        }

        self.wait_queue.wake_all();
        POLL_WAIT_QUEUE.wake_all();

        Ok(client_end)
    }
}

impl FileLike for UnixListener {
    fn stat(&self) -> Result<Stat> {
        Ok(socket_stat(self.inode_no))
    }

    fn accept(&self, options: &OpenOptions) -> Result<(Arc<dyn FileLike>, SockAddr)> {
        // Fast path.
        {
            let mut inner = self.inner.lock();
            if let Some(stream) = inner.backlog.pop_front() {
                let bl_after = inner.backlog.len();
                let sa = stream.getsockname().unwrap_or_else(|_| SockAddr::Un(SockAddrUn {
                    family: AF_UNIX as u16,
                    path: [0u8; 108],
                }));
                drop(inner);
                // Backlog shrunk — POLLIN may have edged from set→clear.
                if self.et_watcher_count.load(Ordering::Relaxed) > 0 {
                    self.state_gen.fetch_add(1, Ordering::Release);
                }
                if crate::fs::epoll::EPOLL_TRACE_FD.load(Ordering::Relaxed) != 0 {
                    info!(
                        "AF_UNIX accept fastpath: listener_inode={} bl_after={}",
                        self.inode_no, bl_after,
                    );
                }
                return Ok((stream as Arc<dyn FileLike>, sa));
            }

            if options.nonblock {
                if crate::fs::epoll::EPOLL_TRACE_FD.load(Ordering::Relaxed) != 0 {
                    static LIM: AtomicU32 = AtomicU32::new(0);
                    if LIM.fetch_add(1, Ordering::Relaxed) < 16 {
                        info!(
                            "AF_UNIX accept EAGAIN: listener_inode={}",
                            self.inode_no,
                        );
                    }
                }
                return Err(Errno::EAGAIN.into());
            }
        }

        // Slow path: wait for incoming connection.
        let result = self.wait_queue.sleep_signalable_until(|| {
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
        });
        if result.is_ok() && self.et_watcher_count.load(Ordering::Relaxed) > 0 {
            self.state_gen.fetch_add(1, Ordering::Release);
        }
        result
    }

    fn getsockname(&self) -> Result<SockAddr> {
        let path_opt = self.path.lock().clone();
        let mut sa = SockAddrUn {
            family: AF_UNIX as u16,
            path: [0u8; 108],
        };
        if let Some(p) = path_opt {
            if let Some(name) = p.strip_prefix('@') {
                // Abstract socket: write NUL prefix + name
                sa.path[0] = 0;
                let bytes = name.as_bytes();
                let len = core::cmp::min(bytes.len(), 106);
                sa.path[1..1 + len].copy_from_slice(&bytes[..len]);
            } else {
                let bytes = p.as_bytes();
                let len = core::cmp::min(bytes.len(), 107);
                sa.path[..len].copy_from_slice(&bytes[..len]);
            }
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

    fn poll_gen(&self) -> u64 {
        // Return 0 when no ET watchers so epoll's poll_cached doesn't
        // freeze on stale state.  See UnixStream::poll_gen comment.
        if self.et_watcher_count.load(Ordering::Relaxed) > 0 {
            self.state_gen.load(Ordering::Acquire)
        } else {
            0
        }
    }
    fn notify_epoll_et(&self, added: bool) {
        if added {
            self.et_watcher_count.fetch_add(1, Ordering::Relaxed);
        } else {
            self.et_watcher_count.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

impl Drop for UnixListener {
    fn drop(&mut self) {
        if let Some(key) = self.key.lock().as_ref() {
            unregister_listener(key);
            // Generation entries are tied to live listeners — drop ours
            // if it's still the one currently published.  If a newer
            // bind has already taken over `(dev, inode_no)`, we leave
            // its generation untouched.
            if let ListenerKey::Inode { dev_id, inode_no, generation } = key {
                release_socket_generation(*dev_id, *inode_no, *generation);
            }
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

struct BoundInfo {
    /// Display path (with `@` prefix for abstract).
    path: String,
    /// Registry key — `Inode(dev,ino)` for filesystem sockets, `Abstract(@name)`
    /// for abstract namespace.  See `ListenerKey` for rationale.
    key: ListenerKey,
}

enum SocketState {
    /// Just created with socket(AF_UNIX, SOCK_STREAM, 0).
    Created,
    /// bind() was called — path is set but not listening yet.
    Bound(BoundInfo),
    /// listen() was called — delegate to UnixListener.
    Listening(Arc<UnixListener>),
    /// connect() was called — delegate to UnixStream.
    Connected(Arc<UnixStream>),
}

pub struct UnixSocket {
    state: SpinLock<SocketState>,
    sock_type: i32,
    /// Pseudo-inode for `/proc/<pid>/fd/N → socket:[INODE]`.  Stays
    /// stable across state transitions (Created → Listening / Connected)
    /// so `lsof`-style tooling can correlate the same fd over time.
    inode_no: u64,
}

impl UnixSocket {
    pub fn new() -> Arc<UnixSocket> {
        Arc::new(UnixSocket {
            state: SpinLock::new(SocketState::Created),
            sock_type: 1, // SOCK_STREAM default
            inode_no: alloc_socket_inode(),
        })
    }

    pub fn new_typed(sock_type: i32) -> Arc<UnixSocket> {
        Arc::new(UnixSocket {
            state: SpinLock::new(SocketState::Created),
            sock_type,
            inode_no: alloc_socket_inode(),
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

/// Extract a path from a SockAddrUn.
/// Supports both filesystem paths (NUL-terminated) and abstract namespace
/// (first byte is \0, remaining bytes are the name — used by D-Bus, X11).
fn sockaddr_un_path(sa: &SockAddrUn) -> Result<&str> {
    if sa.path[0] == 0 {
        // Abstract namespace: "\0<name>" — use bytes 1..nul as the key.
        // We prefix with "@" internally to distinguish from filesystem paths.
        // The name is everything after the leading \0 up to the next \0.
        let end = sa.path[1..].iter().position(|&b| b == 0).map(|p| p + 1).unwrap_or(sa.path.len());
        if end <= 1 {
            return Err(Errno::EINVAL.into());
        }
        core::str::from_utf8(&sa.path[1..end]).map_err(|_| Error::new(Errno::EINVAL))
    } else {
        // Filesystem path: NUL-terminated
        let nul_pos = sa.path.iter().position(|&b| b == 0).unwrap_or(sa.path.len());
        if nul_pos == 0 {
            return Err(Errno::EINVAL.into());
        }
        core::str::from_utf8(&sa.path[..nul_pos]).map_err(|_| Error::new(Errno::EINVAL))
    }
}

impl FileLike for UnixSocket {
    fn stat(&self) -> Result<Stat> {
        Ok(socket_stat(self.inode_no))
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

        let raw_path = sockaddr_un_path(sa)?;
        let is_abstract = sa.path[0] == 0;

        // For filesystem-namespace sockets, create a real on-disk node so
        // that subsequent stat() reports S_IFSOCK, shell tests like
        // `[ -S /tmp/.X11-unix/X0 ]` succeed, and the inode itself is
        // the binding's identity (matches Linux: rename routes follow,
        // unlink breaks connect).  Abstract namespace sockets
        // (sun_path[0] == NUL) have no filesystem presence on Linux
        // either, and are keyed purely by their `@name`.
        let (display_path, key) = if is_abstract {
            let mut s = alloc::string::String::with_capacity(raw_path.len() + 1);
            s.push('@');
            s.push_str(raw_path);
            let key = ListenerKey::Abstract(s.clone());
            (s, key)
        } else {
            use kevlar_vfs::path::Path;
            use kevlar_vfs::stat::{FileMode, GId, UId};

            let proc = crate::process::current_process();
            let root_fs = proc.root_fs();
            let umask = proc.umask();
            let uid = UId::new(proc.uid());
            let gid = GId::new(proc.gid());
            let mode_bits = S_IFSOCK | (0o666 & !umask & 0o7777);

            let (parent_inode, basename) = root_fs
                .lock()
                .lookup_parent_inode(Path::new(raw_path), true)?;
            let parent_dir = parent_inode.as_dir()?.clone();
            let new_inode = match parent_dir.create_file(
                basename,
                FileMode::new(mode_bits),
                uid,
                gid,
            ) {
                Ok(i) => i,
                Err(e) if e.errno() == Errno::EEXIST => {
                    // Path already taken — Linux returns EADDRINUSE here,
                    // not EEXIST.  Common cause: stale socket file from a
                    // previous run that wasn't unlinked.
                    return Err(Errno::EADDRINUSE.into());
                }
                Err(e) => return Err(e),
            };
            let (dev_id, inode_no) = new_inode.inode_key()?;
            let generation = bump_socket_generation(dev_id, inode_no);
            (
                alloc::string::String::from(raw_path),
                ListenerKey::Inode { dev_id, inode_no, generation },
            )
        };

        let mut state = self.state.lock();
        match &*state {
            SocketState::Created => {
                *state = SocketState::Bound(BoundInfo {
                    path: display_path,
                    key,
                });
                Ok(())
            }
            _ => Err(Errno::EINVAL.into()),
        }
    }

    fn listen(&self, backlog: i32) -> Result<()> {
        let mut state = self.state.lock();
        let bound = match &*state {
            SocketState::Bound(info) => BoundInfo {
                path: info.path.clone(),
                key: info.key.clone(),
            },
            _ => return Err(Errno::EINVAL.into()),
        };

        let listener = UnixListener::new();
        *listener.path.lock() = Some(bound.path.clone());
        *listener.key.lock() = Some(bound.key.clone());
        {
            let mut inner = listener.inner.lock();
            inner.max_backlog = core::cmp::min(backlog.max(1) as usize, BACKLOG_MAX);
        }

        register_listener(bound.key, &listener);
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

        let raw_path = sockaddr_un_path(sa)?;
        let is_abstract = sa.path[0] == 0;

        // Resolve the listener key.  For filesystem sockets this means
        // looking up the path's inode — the inode IS the listener's
        // identity, so this matches Linux behavior across rename/unlink:
        //
        //   - bind(/foo)  registers Inode(d,i)
        //   - mv /foo /bar
        //   - connect(/bar) → lookup_path("/bar").inode_key() == (d,i) → finds listener.
        //
        //   - bind(/foo)  registers Inode(d,i)
        //   - unlink(/foo)
        //   - connect(/foo) → lookup_path fails → ENOENT (Linux's exact errno).
        let key = if is_abstract {
            let mut s = alloc::string::String::with_capacity(raw_path.len() + 1);
            s.push('@');
            s.push_str(raw_path);
            ListenerKey::Abstract(s)
        } else {
            use kevlar_vfs::path::Path;
            let proc = crate::process::current_process();
            let pc = proc
                .root_fs()
                .lock()
                .lookup_path(Path::new(raw_path), true)?;
            let (dev_id, inode_no) = pc.inode.inode_key()?;
            // No live binding at this inode → ECONNREFUSED for STREAM
            // (DGRAM falls through to the find_listener miss below and
            // accepts the connect silently, matching Linux's permissive
            // sendto-without-listener behavior).  This catches paths
            // that exist on disk but were never `bind()`ed (e.g. a
            // regular file someone connect()ed to by mistake) AND
            // stale-listener inode-recycle hazards.
            let generation = match current_socket_generation(dev_id, inode_no) {
                Some(g) => g,
                None => {
                    if self.sock_type == 2 {
                        return Ok(());
                    }
                    return Err(Errno::ECONNREFUSED.into());
                }
            };
            ListenerKey::Inode { dev_id, inode_no, generation }
        };

        let listener = match find_listener(&key) {
            Some(l) => l,
            None => {
                // Path resolved (or abstract name was given), but no live
                // listener.  For STREAM sockets this is ECONNREFUSED.
                // For DGRAM sockets, connect() merely sets the default
                // destination — accept silently (systemd's sd_notify
                // does this against possibly-not-yet-listening servers).
                if self.sock_type == 2 {
                    return Ok(());
                }
                return Err(Errno::ECONNREFUSED.into());
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
            SocketState::Bound(info) => {
                let mut sa = SockAddrUn {
                    family: AF_UNIX as u16,
                    path: [0u8; 108],
                };
                if let Some(name) = info.path.strip_prefix('@') {
                    sa.path[0] = 0;
                    let bytes = name.as_bytes();
                    let len = core::cmp::min(bytes.len(), 106);
                    sa.path[1..1 + len].copy_from_slice(&bytes[..len]);
                } else {
                    let bytes = info.path.as_bytes();
                    let len = core::cmp::min(bytes.len(), 107);
                    sa.path[..len].copy_from_slice(&bytes[..len]);
                }
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

    fn poll_gen(&self) -> u64 {
        let state = self.state.lock();
        match &*state {
            SocketState::Listening(l) => l.poll_gen(),
            SocketState::Connected(s) => s.poll_gen(),
            _ => 0,
        }
    }
    fn notify_epoll_et(&self, added: bool) {
        let state = self.state.lock();
        match &*state {
            SocketState::Listening(l) => l.notify_epoll_et(added),
            SocketState::Connected(s) => s.notify_epoll_et(added),
            _ => {}
        }
    }
}

impl fmt::Debug for UnixSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnixSocket").finish()
    }
}
