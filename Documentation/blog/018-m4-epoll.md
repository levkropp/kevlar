# Milestone 4 Begins: Epoll for systemd

Kevlar can now boot BusyBox, run bash, and beat Linux on core syscall
benchmarks. The next major goal is booting systemd — the init system used
by most Linux distributions. This is Milestone 4, and it starts with epoll.

## Why epoll first

systemd's main loop is an epoll event loop. Before it reads a config file
or starts a service, it calls `epoll_create1`, adds signal, timer, and
notification fds, and enters `epoll_wait`. Without epoll, systemd cannot
even begin initialization.

We already had `poll(2)` and `select(2)`, both backed by a global
`POLL_WAIT_QUEUE` that wakes sleeping tasks when any fd state changes.
Epoll reuses this same infrastructure — there's no per-fd callback
registration or O(1) readiness tracking. On each wakeup, `epoll_wait`
re-polls all interested fds. This is O(n) per wakeup, but n is ~10 fds
for systemd's event loop, so correctness matters more than scalability.

## The implementation

### EpollInstance as a FileLike

An epoll fd is itself a file descriptor — you can `fstat` it, `close` it,
and even add it to another epoll instance (nested epoll). We implement
this by making `EpollInstance` implement the `FileLike` trait:

```rust
pub struct EpollInstance {
    interests: SpinLock<BTreeMap<i32, Interest>>,
}

struct Interest {
    file: Arc<dyn FileLike>,  // keep-alive reference
    events: u32,               // EPOLLIN, EPOLLOUT, etc.
    data: u64,                 // opaque user data
}
```

The `FileLike` impl provides `stat()` (returns zeroed metadata) and
`poll()` (returns POLLIN if any child fd is ready — enabling nested epoll).

### Downcast for type recovery

When `epoll_ctl` receives an epoll fd number, it needs to get the
`EpollInstance` back from the fd table, which stores `Arc<dyn FileLike>`.
Rust's `Any` trait handles this via the `Downcastable` supertrait:

```rust
let epoll_file = table.get(epfd)?.as_file()?;
let epoll = epoll_file.as_any().downcast_ref::<EpollInstance>()
    .ok_or(Error::new(Errno::EINVAL))?;
```

If the fd isn't actually an epoll instance, we return EINVAL — same as
Linux.

### Safe packed struct serialization

Linux's `struct epoll_event` is packed (12 bytes: u32 + u64 with no
padding). Our kernel crate enforces `#![deny(unsafe_code)]`, so we can't
use `ptr::read_unaligned`. Instead, we serialize/deserialize at the byte
level:

```rust
impl EpollEvent {
    fn from_bytes(b: &[u8; 12]) -> EpollEvent {
        let events = u32::from_ne_bytes([b[0], b[1], b[2], b[3]]);
        let data = u64::from_ne_bytes([b[4], b[5], b[6], b[7],
                                       b[8], b[9], b[10], b[11]]);
        EpollEvent { events, data }
    }

    fn to_bytes(&self) -> [u8; 12] {
        let mut buf = [0u8; 12];
        buf[0..4].copy_from_slice(&self.events.to_ne_bytes());
        buf[4..12].copy_from_slice(&self.data.to_ne_bytes());
        buf
    }
}
```

Zero `unsafe`, same ABI.

### epoll_wait blocking

`epoll_wait` uses the same `sleep_signalable_until` pattern as our
existing `poll(2)` — a closure that returns `Some(result)` when ready or
`None` to keep sleeping:

```rust
let ready_events = POLL_WAIT_QUEUE.sleep_signalable_until(|| {
    if timeout > 0 && started_at.elapsed_msecs() >= timeout as usize {
        return Ok(Some(Vec::new()));  // timeout
    }
    let mut events = Vec::new();
    let count = epoll.collect_ready(&mut events, maxevents);
    if count > 0 {
        Ok(Some(events))
    } else if timeout == 0 {
        Ok(Some(Vec::new()))  // non-blocking
    } else {
        Ok(None)  // keep sleeping
    }
})?;
```

`epoll_pwait` dispatches to the same handler — the signal mask argument
is ignored for now, which is sufficient for initial systemd bringup.

## Syscall numbers

| Syscall | x86_64 | ARM64 |
|---------|--------|-------|
| epoll_create1 | 291 | 20 |
| epoll_ctl | 233 | 21 |
| epoll_wait | 232 | (n/a) |
| epoll_pwait | 281 | 22 |

ARM64 only has `epoll_pwait`, not the older `epoll_wait`.

## What's next

Epoll is the event loop shell. Phase 2 fills it with the event sources
systemd actually monitors: `signalfd` (SIGCHLD delivery as fd reads),
`timerfd` (scheduled wakeups), and `eventfd` (internal notifications).
Together with epoll, these four primitives form the complete I/O
multiplexing substrate that systemd's main loop requires.
