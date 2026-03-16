# Phase 2: Event Source FDs

**Goal:** Implement signalfd, timerfd, and eventfd — the three fd-based event
sources that systemd plugs into its epoll loop.

**Prerequisite:** Phase 1 (Pollable trait + epoll).

## Syscalls

| Syscall | Number | Priority | Notes |
|---------|--------|----------|-------|
| `signalfd4` | 289 | Required | Deliver signals as readable fd events |
| `timerfd_create` | 283 | Required | Create timer fd |
| `timerfd_settime` | 286 | Required | Arm/disarm timer |
| `timerfd_gettime` | 287 | Nice-to-have | Query remaining time |
| `eventfd2` | 290 | Required | Inter-process/thread notification counter |

## Design

### signalfd

Redirects specified signals from the normal delivery path into a readable fd.
systemd uses this to handle SIGCHLD, SIGTERM, SIGHUP via epoll instead of
signal handlers.

```rust
struct SignalFd {
    mask: SigSet,           // which signals this fd captures
    pending: VecDeque<SignalFdSiginfo>,  // queued signal info structs
    wait_queue: WaitQueue,  // wake epoll/read when signal arrives
}
```

**Integration with signal delivery:** In `try_delivering_signal()`, before
dispatching to a handler, check if the signal is in any signalfd's mask. If so,
queue a `signalfd_siginfo` struct (128 bytes) to the signalfd instead of
delivering normally.

**Pollable:** Returns `PollEvent::IN` when `pending` is non-empty.

**Read:** Returns one or more `signalfd_siginfo` structs (128 bytes each).
Blocks if no signals pending (unless O_NONBLOCK).

### timerfd

Creates an fd that becomes readable when a timer expires. systemd uses this
for scheduled service starts, watchdog timers, and rate limiting.

```rust
struct TimerFd {
    clockid: i32,           // CLOCK_MONOTONIC or CLOCK_REALTIME
    armed: Option<TimerSpec>,
    expirations: u64,       // number of times fired since last read
    wait_queue: WaitQueue,
}

struct TimerSpec {
    initial: Duration,      // time until first expiration
    interval: Duration,     // repeat interval (0 = one-shot)
    next_fire: u64,         // absolute nanoseconds for next expiration
}
```

**Timer tick integration:** On each timer interrupt (TICK_HZ=100), check all
armed timerfds. If `now >= next_fire`, increment `expirations` and wake the
wait queue. For interval timers, compute next fire time.

**Pollable:** Returns `PollEvent::IN` when `expirations > 0`.

**Read:** Returns expirations count as u64 (8 bytes), resets to 0. Blocks
if expirations == 0 (unless O_NONBLOCK).

### eventfd

A simple counter fd for inter-process notification. systemd uses this for
internal wake-up signaling between components.

```rust
struct EventFd {
    counter: u64,
    flags: EventFdFlags,    // EFD_SEMAPHORE, EFD_NONBLOCK
    wait_queue: WaitQueue,
}
```

**Write:** Adds value to counter (blocks if would overflow u64::MAX - 1).
**Read:** Returns counter and resets to 0. With EFD_SEMAPHORE, returns 1 and
decrements. Blocks if counter == 0 (unless O_NONBLOCK).
**Pollable:** Returns `PollEvent::IN` when counter > 0, `PollEvent::OUT`
when counter < u64::MAX - 1.

## Files to Create/Modify

- `kernel/fs/signalfd.rs` (NEW) — SignalFd struct, read, Pollable impl
- `kernel/fs/timerfd.rs` (NEW) — TimerFd struct, settime/gettime, Pollable
- `kernel/fs/eventfd.rs` (NEW) — EventFd struct, read/write, Pollable
- `kernel/process/signal.rs` — Hook signalfd into signal delivery path
- `kernel/timer.rs` or equivalent — timerfd expiration check on tick
- `kernel/syscalls/signalfd.rs` (NEW)
- `kernel/syscalls/timerfd.rs` (NEW)
- `kernel/syscalls/eventfd.rs` (NEW)
- `kernel/syscalls/mod.rs` — dispatch entries

## Integration Test

```c
// Test: signalfd + timerfd + eventfd all monitored via epoll
int epfd = epoll_create1(0);

// signalfd: block SIGCHLD, read via fd
sigset_t mask;
sigemptyset(&mask);
sigaddset(&mask, SIGCHLD);
sigprocmask(SIG_BLOCK, &mask, NULL);
int sfd = signalfd(-1, &mask, SFD_NONBLOCK);

// timerfd: fire in 10ms
int tfd = timerfd_create(CLOCK_MONOTONIC, 0);
struct itimerspec ts = { .it_value = { .tv_nsec = 10000000 } };
timerfd_settime(tfd, 0, &ts, NULL);

// eventfd
int efd = eventfd(0, 0);

// Add all to epoll
struct epoll_event ev;
ev.events = EPOLLIN; ev.data.fd = sfd;
epoll_ctl(epfd, EPOLL_CTL_ADD, sfd, &ev);
ev.data.fd = tfd;
epoll_ctl(epfd, EPOLL_CTL_ADD, tfd, &ev);
ev.data.fd = efd;
epoll_ctl(epfd, EPOLL_CTL_ADD, efd, &ev);

// Write to eventfd from child, which also generates SIGCHLD
if (fork() == 0) {
    uint64_t val = 1;
    write(efd, &val, sizeof(val));
    _exit(0);
}

// Should get 3 events: timerfd expiry, eventfd write, signalfd SIGCHLD
struct epoll_event events[3];
int n = epoll_wait(epfd, events, 3, 1000);
assert(n >= 2);
printf("TEST_PASS event_fds (%d events)\n", n);
```

## Reference

- Linux man pages: signalfd(2), timerfd_create(2), eventfd(2)

## Estimated Complexity

~600-800 lines. signalfd has the trickiest integration (hooks into signal
delivery). timerfd needs timer interrupt integration. eventfd is simplest.
