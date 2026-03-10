# Unix Domain Sockets: D-Bus Transport Layer

Blog 019 gave Kevlar the event source fds that systemd plugs into epoll.
But systemd's main business — managing services — happens over D-Bus, and
D-Bus runs on Unix domain sockets. This post covers the AF_UNIX socket
implementation that completes the systemd I/O foundation.

## The state machine

A Unix socket transitions through states depending on which syscalls
are called on it:

```
socket() → Created
  ↓ bind()          ↓ connect()
Bound             Connected (bidirectional stream)
  ↓ listen()
Listening (accept incoming connections)
```

The kernel represents this as an enum inside a SpinLock, which means one
`Arc<UnixSocket>` can transition from Created → Bound → Listening without
changing identity in the fd table:

```rust
enum SocketState {
    Created,
    Bound(String),
    Listening(Arc<UnixListener>),
    Connected(Arc<UnixStream>),
}
```

Each `FileLike` method checks the current state and delegates to the
appropriate inner type. Read/write on a Listening socket returns EINVAL.
Connect on an already-Connected socket replaces the stream.

## Named sockets and the listener registry

When a process calls `bind("/run/dbus/system_bus_socket")` followed by
`listen()`, the kernel needs a way for a different process's `connect()`
to find that listener. We use a simple global registry:

```rust
static UNIX_LISTENERS: SpinLock<VecDeque<(String, Arc<UnixListener>)>> =
    SpinLock::new(VecDeque::new());
```

`connect()` looks up the path, calls `enqueue_connection()` on the
listener, and gets back the client end of a new stream pair. The listener
pushes the server end into its backlog. `accept()` pops from the backlog.

This is simpler than creating actual socket inodes in the VFS — we skip
filesystem integration entirely. The path is just a lookup key. For
systemd's use case (well-known paths like `/run/dbus/system_bus_socket`),
this is sufficient.

## Connected streams: shared ring buffers

A connected Unix stream pair is two `RingBuffer<u8, 65536>` with
crossed references — each end's `tx` is the other end's `rx`:

```rust
pub struct UnixStream {
    tx: Arc<SpinLock<StreamInner>>,  // our write buffer
    rx: Arc<SpinLock<StreamInner>>,  // peer's write buffer
    peer_closed: Arc<AtomicBool>,
}

fn new_pair() -> (Arc<UnixStream>, Arc<UnixStream>) {
    let buf_a = Arc::new(SpinLock::new(StreamInner { ... }));
    let buf_b = Arc::new(SpinLock::new(StreamInner { ... }));
    // a.tx = buf_a, a.rx = buf_b
    // b.tx = buf_b, b.rx = buf_a
}
```

The read/write implementation follows the same pattern as pipes: fast
path under lock, slow path via `POLL_WAIT_QUEUE.sleep_signalable_until`.
EOF detection uses both `shut_wr` (explicit shutdown) and `peer_closed`
(the peer's Arc was dropped).

## SCM_RIGHTS: passing file descriptors between processes

D-Bus uses `sendmsg`/`recvmsg` with `SCM_RIGHTS` ancillary data to pass
file descriptors between processes. The mechanism:

1. **sendmsg**: parse `struct msghdr` and its `cmsghdr` chain from
   userspace. For each `SCM_RIGHTS` cmsg, look up the sender's fds,
   clone their `Arc<OpenedFile>`, and attach them to the stream's
   ancillary queue.

2. **recvmsg**: after reading data, check for pending ancillary data.
   For each `SCM_RIGHTS` cmsg, install the `Arc<OpenedFile>` into the
   receiver's fd table and write the new fd numbers back to userspace.

The ancillary data queue is a `VecDeque<AncillaryData>` inside each
stream direction's `StreamInner`. This decouples the ancillary data
from the byte stream — a received cmsg is associated with the next
`recvmsg` call, not with a specific byte offset.

```rust
pub enum AncillaryData {
    Rights(Vec<Arc<OpenedFile>>),
}
```

## accept4 and setsockopt

`accept4` extends `accept` with `SOCK_CLOEXEC` and `SOCK_NONBLOCK`
flags applied to the new fd. We refactored `sys_accept` to delegate to
`sys_accept4` with flags=0.

`setsockopt` is a stub that silently accepts the options systemd and
D-Bus set: `SO_REUSEADDR`, `SO_PASSCRED`, `SO_KEEPALIVE`, `TCP_NODELAY`,
and buffer size options. None of these affect behavior yet.

## What's next

With Unix domain sockets, Kevlar has the complete transport layer for
D-Bus. Phase 4 adds the remaining syscall stubs that systemd needs
before its main loop — `socketpair`, inotify, and the various `prctl`
and `fcntl` options that systemd probes on startup.
