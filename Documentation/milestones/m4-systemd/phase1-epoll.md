# Phase 1: Epoll

**Goal:** Implement the epoll event multiplexing subsystem — the foundation of
systemd's main loop and every modern Linux server.

## Syscalls

| Syscall | Number | Priority | Notes |
|---------|--------|----------|-------|
| `epoll_create1` | 291 | Required | Create epoll fd with EPOLL_CLOEXEC |
| `epoll_ctl` | 233 | Required | Add/modify/delete interest entries |
| `epoll_wait` | 232 | Required | Wait for events (timeout in ms) |
| `epoll_pwait` | 281 | Nice-to-have | epoll_wait + signal mask (can alias epoll_wait initially) |

## Design

### The Pollable Trait

Before building epoll, we need a generic readiness interface that all fd types
implement. This is the most important design decision in Phase 1:

```rust
/// Readiness events (matches Linux EPOLLIN/EPOLLOUT/EPOLLHUP/EPOLLERR bits).
bitflags! {
    pub struct PollEvent: u32 {
        const IN    = 0x001;  // readable
        const OUT   = 0x004;  // writable
        const ERR   = 0x008;  // error condition
        const HUP   = 0x010;  // hang up
        const RDHUP = 0x2000; // peer closed write half
    }
}

/// Trait for file descriptions that can report readiness.
pub trait Pollable {
    /// Return current readiness (non-blocking poll).
    fn poll(&self) -> PollEvent;

    /// Register a waker to be notified when readiness changes.
    /// Returns a token that can be used to unregister.
    fn register_waker(&self, waker: Arc<EpollWaker>) -> WakerToken;

    /// Unregister a previously registered waker.
    fn unregister_waker(&self, token: WakerToken);
}
```

Every fd type that can be polled implements this: pipes, sockets, signalfd,
timerfd, eventfd, /dev/null, ttys.

### Epoll Internal Structure

```rust
struct EpollInstance {
    /// Interest list: fds being monitored.
    interests: SpinLock<HashMap<Fd, EpollInterest>>,
    /// Ready list: fds with pending events.
    ready: SpinLock<Vec<(Fd, PollEvent)>>,
    /// Wait queue for epoll_wait callers.
    wait_queue: WaitQueue,
}

struct EpollInterest {
    fd: Fd,
    events: PollEvent,    // what events to watch
    data: u64,            // user-provided epoll_data
    waker_token: WakerToken,
}
```

### Waker Notification Flow

```
1. Process calls epoll_ctl(ADD, fd, events) →
   - Create EpollInterest
   - Call fd.register_waker(epoll_waker) → get token
   - Store interest + token

2. Something writes to a pipe →
   - Pipe's write() calls waker.wake(PollEvent::IN)
   - Waker adds (fd, events) to epoll's ready list
   - Waker calls epoll.wait_queue.wake_all()

3. Process calls epoll_wait() →
   - If ready list non-empty: drain and return
   - If empty: sleep on wait_queue until woken
```

### Edge vs Level Triggering

- **Level-triggered (default):** fd stays in ready list as long as it's
  readable/writable. Re-added after each epoll_wait return.
- **Edge-triggered (EPOLLET):** fd only added to ready list on state
  *transitions*. Not re-added automatically.
- **Start with level-triggered only.** EPOLLET can be added later; systemd
  doesn't require it for basic operation.

## Files to Create/Modify

- `kernel/fs/epoll.rs` (NEW) — EpollInstance, epoll_create/ctl/wait
- `kernel/fs/poll.rs` (NEW) — PollEvent, Pollable trait, EpollWaker
- `kernel/fs/pipe.rs` — implement Pollable for PipeReader/PipeWriter
- `kernel/net/socket.rs` — implement Pollable for sockets
- `kernel/fs/devfs/tty.rs` — implement Pollable for TTY
- `kernel/fs/devfs/null.rs` — implement Pollable for /dev/null
- `kernel/syscalls/mod.rs` — dispatch entries
- `kernel/syscalls/epoll.rs` (NEW) — syscall handlers

## Integration Test

```c
// integration_tests/test_epoll.c
// Create pipe, add read end to epoll, write from child, epoll_wait in parent.
int epfd = epoll_create1(0);
int pipefd[2];
pipe(pipefd);

struct epoll_event ev = { .events = EPOLLIN, .data.fd = pipefd[0] };
epoll_ctl(epfd, EPOLL_CTL_ADD, pipefd[0], &ev);

if (fork() == 0) {
    write(pipefd[1], "x", 1);
    _exit(0);
}

struct epoll_event events[1];
int n = epoll_wait(epfd, events, 1, 1000);
assert(n == 1 && (events[0].events & EPOLLIN));
printf("TEST_PASS epoll\n");
```

## Reference

- FreeBSD: `sys/compat/linux/linux_event.c` (epoll emulation via kqueue)
- Linux: `fs/eventpoll.c` (canonical implementation)
- OSv: `core/epoll.cc` (~380 lines, clean design)
- Man pages: epoll(7), epoll_create(2), epoll_ctl(2), epoll_wait(2)

## Estimated Complexity

~500-700 lines of new code. The Pollable trait design is the critical path —
get it right here and Phases 2-3 become straightforward.
