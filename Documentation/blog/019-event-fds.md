# Event Source FDs: Filling the Epoll Loop

Blog 018 gave Kevlar an epoll event loop. But an empty loop is useless —
systemd needs event sources to monitor. This post covers the three fd
types that systemd plugs into epoll before it does anything else:
signalfd, timerfd, and eventfd.

## eventfd: the simplest possible IPC

An eventfd is a counter wrapped in a file descriptor. Write adds to the
counter, read returns it and resets to zero. Poll reports POLLIN when the
counter is non-zero. systemd uses this for internal wake-up signaling
between components.

```rust
pub struct EventFd {
    inner: SpinLock<EventFdInner>,
}

struct EventFdInner {
    counter: u64,
    semaphore: bool,  // EFD_SEMAPHORE: read returns 1, decrements
}
```

The implementation follows the same pattern as pipes: fast path tries the
operation under lock, falls back to `POLL_WAIT_QUEUE.sleep_signalable_until`
for blocking. Write blocks only if the counter would overflow `u64::MAX - 1`
(effectively never in practice).

## timerfd: lazy expiration checking

A timerfd becomes readable when a deadline passes. systemd uses this for
scheduled service starts, watchdog timers, and rate limiting.

The obvious implementation would hook into the timer interrupt to check
armed timerfds on every tick. We chose a simpler approach: **lazy
evaluation**. The timerfd stores an absolute nanosecond deadline, and
`poll()`/`read()` compare it against the current monotonic clock:

```rust
fn check_expiry(inner: &mut TimerFdInner) {
    if inner.next_fire_ns == 0 { return; }  // disarmed

    let now_ns = timer::read_monotonic_clock().nanosecs() as u64;
    if now_ns < inner.next_fire_ns { return; }  // not yet

    if inner.interval_ns > 0 {
        // Periodic: count elapsed intervals
        let elapsed = now_ns - inner.next_fire_ns;
        let extra = elapsed / inner.interval_ns;
        inner.expirations += 1 + extra;
        inner.next_fire_ns += (1 + extra) * inner.interval_ns;
    } else {
        // One-shot
        inner.expirations += 1;
        inner.next_fire_ns = 0;
    }
}
```

This is correct because epoll_wait re-polls all interested fds on every
wakeup. The question is: what causes the wakeup? Without something
periodically nudging the wait queue, a sleeping epoll_wait would never
notice the timer expired.

The fix: `handle_timer_irq()` now calls `POLL_WAIT_QUEUE.wake_all()` on
every tick (100 Hz on x86_64). This costs one atomic load per tick when
nobody is waiting (the fast path checks `waiter_count`), and at most one
reschedule per tick when someone is. This also fixes a latent bug where
`poll()`/`select()` timeouts were unreliable — they depended on some other
event waking the queue.

## signalfd: zero modifications to signal delivery

signalfd was the design challenge. systemd uses it to handle SIGCHLD,
SIGTERM, and SIGHUP through epoll instead of signal handlers. The normal
approach would intercept signal delivery, check if a signalfd is
watching, and redirect the signal. This would require threading signalfd
state through the signal delivery path.

We chose a simpler design: **don't touch signal delivery at all**. The
user blocks signals via `sigprocmask`, creates a signalfd with the same
mask, and adds it to epoll. Blocked signals accumulate in the process's
existing `pending` bitmask. The signalfd's `poll()` and `read()` simply
check this bitmask:

```rust
fn poll(&self) -> Result<PollStatus> {
    let pending = current_process().signal_pending_bits();
    if pending & self.mask != 0 {
        Ok(PollStatus::POLLIN)
    } else {
        Ok(PollStatus::empty())
    }
}
```

On read, `pop_pending_masked(mask)` atomically dequeues matching signals
and fills in 128-byte `signalfd_siginfo` structs. No new data
structures, no hooks, no coordination — just reading from state that
already exists.

For epoll to notice new signals promptly, `send_signal()` now calls
`POLL_WAIT_QUEUE.wake_all()` after queuing a signal.

## Fixing a signal delivery bug

While implementing signalfd, we found a bug in `try_delivering_signal`.
The old code called `pop_pending()` which unconditionally removed the
lowest-numbered pending signal, then checked if it was blocked:

```rust
// BEFORE (buggy): blocked signals are popped and silently discarded
let (signal, action) = sigs.pop_pending();
if !sigset.is_blocked(signal) {
    // deliver
}
// If blocked: signal is gone forever
```

The fix: `pop_pending_unblocked(sigset)` only pops signals that aren't
in the blocked set. Blocked signals remain pending for signalfd to
consume or for later delivery when unblocked.

We also fixed `has_pending_signals()` — used by `sleep_signalable_until`
to decide whether to return EINTR — to check `pending & ~blocked`
instead of just `pending != 0`. Without this, blocked signals would
cause spurious EINTR returns from every blocking syscall.

## What's next

With epoll + signalfd + timerfd + eventfd, Kevlar has the complete I/O
multiplexing substrate for systemd's main loop. Phase 3 tackles Unix
domain sockets — the transport layer for D-Bus, which systemd uses for
inter-process communication with every service it manages.
