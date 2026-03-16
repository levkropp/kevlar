# Networking

## TCP/IP: smoltcp

Kevlar uses [smoltcp 0.12](https://github.com/smoltcp-rs/smoltcp) for the TCP/IP
stack. smoltcp is a no\_std, event-driven network stack that runs entirely inside the
kernel without its own thread.

The network stack is accessed through the `NetworkStackService` trait (Ring 2 boundary):

```rust
pub trait NetworkStackService: Send + Sync {
    fn create_tcp_socket(&self) -> Result<Arc<dyn FileLike>>;
    fn create_udp_socket(&self) -> Result<Arc<dyn FileLike>>;
    fn create_unix_socket(&self) -> Result<Arc<dyn FileLike>>;
    fn create_icmp_socket(&self) -> Result<Arc<dyn FileLike>>;
    fn process_packets(&self);
}
```

Under Fortress/Balanced profiles, calls go through `call_service(catch_unwind)`.
Under Performance/Ludicrous, the `SmoltcpNetworkStack` is called directly as a
concrete type (inlined, no vtable dispatch).

### Packet Processing

Incoming packets from the VirtIO driver are queued in a lock-free `ArrayQueue<Vec<u8>>`
(128 packets max). The processing loop runs from timer interrupt context:

```rust
loop {
    match iface.poll(timestamp, &mut device, &mut sockets) {
        PollResult::None => break,
        PollResult::SocketStateChanged => {}
    }
}
SOCKET_WAIT_QUEUE.wake_all();
POLL_WAIT_QUEUE.wake_all();
```

### Network Configuration

- **DHCP**: smoltcp's built-in DHCP client acquires an IP address and gateway at boot.
- **Static**: Fixed IP/mask/gateway from kernel parameters.

## Socket Types

| Domain | Type | Protocol | Implementation |
|--------|------|----------|---------------|
| `AF_INET` | `SOCK_STREAM` | TCP | `TcpSocket` via smoltcp |
| `AF_INET` | `SOCK_DGRAM` | UDP | `UdpSocket` via smoltcp |
| `AF_INET` | `SOCK_DGRAM` | ICMP | `IcmpSocket` via smoltcp |
| `AF_UNIX` | `SOCK_STREAM` | — | `UnixSocket` (in-kernel) |
| `AF_UNIX` | `SOCK_DGRAM` | — | `UnixSocket` (in-kernel) |

Not supported: `AF_INET6` (IPv6), `AF_NETLINK` (returns `EAFNOSUPPORT` so tools fall
back to ioctl-based configuration), `AF_PACKET`, `SOCK_RAW`, `SOCK_SEQPACKET`.

### TCP

```rust
pub struct TcpSocket {
    handle: SocketHandle,
    local_endpoint: AtomicCell<Option<IpEndpoint>>,
    backlogs: SpinLock<Vec<Arc<TcpSocket>>>,
    num_backlogs: AtomicUsize,
}
```

- Listen backlog: up to 8 pre-allocated sockets per listener.
- Auto port assignment: starting at port 50000.
- `accept()` blocks on `SOCKET_WAIT_QUEUE` until a backlog socket completes the
  three-way handshake.
- Buffer sizes: 4 KB RX + 4 KB TX per socket.

### UDP

```rust
pub struct UdpSocket {
    handle: SocketHandle,
    peer: SpinLock<Option<IpEndpoint>>,  // Set by connect()
}
```

- `sendto` uses the destination from the `sockaddr` argument or the connected peer.
- `recvfrom` returns the source endpoint in metadata.
- Auto-bind on first send if not explicitly bound.

### ICMP

```rust
pub struct IcmpSocket {
    handle: SocketHandle,
    ident: SpinLock<u16>,
}
```

Used by BusyBox `ping`. Auto-binds with a pseudo-random identifier on first send.
Sends and receives raw ICMP echo request/reply packets.

## Unix Domain Sockets

Unix domain sockets (`AF_UNIX`) use a state machine pattern:

```
UnixSocket (Created)
  ├── bind() → Bound
  │     └── listen() → Listening (UnixListener)
  └── connect() → Connected (UnixStream)
```

### UnixStream

A bidirectional pipe pair. Each direction has a 16 KB ring buffer:

```rust
// Each end owns a tx buffer; peer reads from it
pub struct UnixStream {
    tx: SpinLock<RingBuffer<u8, 16384>>,
    rx: Arc<SpinLock<RingBuffer<u8, 16384>>>,  // = peer's tx
    ancillary: SpinLock<VecDeque<AncillaryData>>,
    // ...
}
```

### UnixListener

Accepts incoming connections from a backlog queue (max 128):

```rust
pub struct UnixListener {
    backlog: SpinLock<VecDeque<Arc<UnixStream>>>,
    wait_queue: WaitQueue,
}
```

A global listener registry maps filesystem paths to `UnixListener` instances.
`connect()` searches this registry to find the listener.

### SCM\_RIGHTS (File Descriptor Passing)

`sendmsg` with `SCM_RIGHTS` ancillary data sends file descriptors across a Unix
socket. The sender's `Arc<OpenedFile>` references are queued on the stream:

```rust
pub enum AncillaryData {
    Rights(Vec<Arc<OpenedFile>>),
}
```

`recvmsg` installs the received file references into the receiver's file descriptor
table and returns the new fd numbers in the control message.

## epoll

`epoll_create1`, `epoll_ctl`, and `epoll_wait` are fully implemented:

```rust
pub struct EpollInstance {
    interests: SpinLock<BTreeMap<i32, Interest>>,
}

struct Interest {
    file: Arc<dyn FileLike>,
    events: u32,  // EPOLLIN, EPOLLOUT, EPOLLERR, EPOLLHUP
    data: u64,
}
```

`epoll_wait` polls all registered interests and returns ready ones. For `timeout > 0`,
it sleeps on `POLL_WAIT_QUEUE` and re-polls on wakeup. Level-triggered mode only.

The O(n) poll approach is acceptable for typical use (systemd/OpenRC watch ~10 fds).

## sendfile

`sendfile(out_fd, in_fd, offset, count)` reads 4 KB chunks from the input file and
writes them to the output socket/file. Uses an intermediate kernel buffer (not
zero-copy).

## Socket Options

Most socket options are accepted silently for compatibility but not enforced:

| Level | Options | Status |
|-------|---------|--------|
| `SOL_SOCKET` | `SO_ERROR`, `SO_TYPE`, `SO_RCVBUF`, `SO_SNDBUF` | Read (real values) |
| `SOL_SOCKET` | `SO_REUSEADDR`, `SO_KEEPALIVE`, `SO_PASSCRED`, `SO_REUSEPORT` | Write (stub) |
| `IPPROTO_TCP` | `TCP_NODELAY` | Write (stub) |

## VirtIO-Net Driver

The `VirtioNet` driver (`exts/virtio_net/`) communicates with QEMU's virtio-net device:

- Supports both modern (12-byte header) and legacy (10-byte header) VirtIO modes.
- RX queue: pre-allocated 2048-byte descriptors, replenished on IRQ.
- TX queue: on-demand transmission with dual descriptors (header + payload).
- Implements `EthernetDriver` trait consumed by the smoltcp integration layer.

## Socket API Summary

| Syscall | Support |
|---|---|
| `socket` | AF\_INET (TCP/UDP/ICMP), AF\_UNIX |
| `bind` | IP address + port, Unix path |
| `connect` | TCP three-way handshake, Unix stream |
| `listen` / `accept` | TCP and Unix listeners |
| `send` / `recv` | Basic send/receive |
| `sendto` / `recvfrom` | UDP datagrams, ICMP |
| `sendmsg` / `recvmsg` | SCM\_RIGHTS fd passing |
| `setsockopt` / `getsockopt` | See table above |
| `shutdown` | TCP half-close, Unix stream |
| `getsockname` / `getpeername` | Local and remote address |
| `socketpair` | AF\_UNIX pairs |
| `poll` / `epoll` | Readiness monitoring |
| `sendfile` | File-to-socket transfer |
