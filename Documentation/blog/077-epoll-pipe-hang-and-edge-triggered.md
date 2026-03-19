# 077: Three Bugs in Twelve Lines — Fixing the Epoll Pipe Hang

After the 076 contract test expansion, two epoll tests remained broken:
`epoll_level` (XFAIL, 30-second timeout) and `epoll_edge` (FAIL, wrong
semantics). The minimal reproducer was deceptively simple:

```c
int ep = epoll_create1(0);
int fds[2]; pipe(fds);
struct epoll_event ev = {.events = EPOLLIN, .data.fd = fds[0]};
epoll_ctl(ep, EPOLL_CTL_ADD, fds[0], &ev);
write(fds[1], "abc", 3);
char buf; read(fds[0], &buf, 1); // HANGS
```

Adding a pipe to an epoll instance, then reading from the pipe, hung
forever. Without the `epoll_ctl`, pipe read worked fine. Three independent
bugs conspired to create this behavior.

## Bug 1: Ring buffer infinite loop

The primary hang was in `PipeReader::read()`. After reading the requested
byte, the pipe read loop continued calling `pop_slice(0)` (since
`remaining_len()` was 0), which returned `Some(empty_slice)` instead of
`None`, spinning forever:

```rust
while let Some(src) = pipe.buf.pop_slice(writer.remaining_len()) {
    writer.write_bytes(src)?;  // writes 0 bytes, remaining stays 0
}
// Never reaches here
```

The fix in `ring_buffer.rs` was one line:

```rust
if !self.is_readable() || len == 0 {
    return None;
}
```

While fixing this, we also found the else branch in `pop_slice` used
`self.wp` (write pointer) instead of `self.rp` (read pointer) for the
wrapped-buffer case — a latent data corruption bug that would trigger once
a pipe's 4KB ring buffer wrapped around:

```rust
// Before (wrong): returned data from write position
self.wp..min(self.wp + len, CAP)
// After (correct): return data from read position
self.rp..min(self.rp + len, CAP)
```

## Bug 2: EPOLL_CTL_DEL rejected NULL event pointer

With the ring buffer fixed, `epoll_level` progressed through 4 of 6
checks before failing at `after_del` — deleting an fd from epoll had no
effect.

The cause: the syscall dispatch did `UserVAddr::new_nonnull(a4)?` for the
event pointer argument. Linux allows NULL for `EPOLL_CTL_DEL` (the event
pointer is ignored), but Kevlar returned EFAULT before the handler was
even called. The C test didn't check the return value:

```c
epoll_ctl(ep, EPOLL_CTL_DEL, fds[0], NULL);  // silently failed
```

Fix: changed the dispatch to pass `UserVAddr::new(a4)` (returns
`Option<UserVAddr>`), and `sys_epoll_ctl` validates non-null only for
ADD/MOD:

```rust
let event = if op != EPOLL_CTL_DEL {
    let ptr = event_ptr.ok_or(Error::new(Errno::EFAULT))?;
    // ...
```

## Bug 3: Inconsistent lock discipline in epoll

`sys_epoll_ctl` used `opened_files().lock()` (with `cli`) while every
other fd table access used `opened_files_no_irq()`. The `interests` lock
inside `add()/modify()/delete()` also used `lock()` unnecessarily. Changed
all to `lock_no_irq()` since neither the fd table nor the interests map is
accessed from interrupt context.

## Edge-triggered (EPOLLET) support

With all three bugs fixed, `epoll_level` passed. `epoll_edge` still failed
at `no_refire` — the edge-triggered mode wasn't implemented at all;
Kevlar treated EPOLLET the same as level-triggered.

The challenge: Linux implements EPOLLET using per-fd waitqueue callbacks.
When a file's state changes, it wakes the epoll instance directly. Kevlar
uses a simpler architecture — a global `POLL_WAIT_QUEUE` woken by the
timer at 100 Hz, with epoll re-polling all interests on each wake. There
are no per-fd callbacks, so we can't directly observe state transitions.

The problem this creates: if a pipe goes readable → empty → readable
between two `epoll_wait` calls (user reads all data, then writes new
data), we see "readable" both times. Without observing the intermediate
empty state, we can't detect the new edge.

### Generation counters

The solution: a monotonically increasing generation counter on each
pollable file. Every state change (read, write, close) increments the
counter. The ET interest stores the generation at which it last reported.
If the current generation differs, something changed — fire the edge.

```rust
// In PipeShared:
state_gen: AtomicU64,  // starts at 1, incremented on every state change

// In Interest:
last_gen: AtomicU64,   // 0 = never reported

// In check_interest():
let cur_gen = interest.file.poll_gen();
if cur_gen == 0 { return true; }     // file doesn't track — fall back to LT
if cur_gen == interest.last_gen.load(Relaxed) {
    return false;                     // same generation — suppress
}
interest.last_gen.store(cur_gen, Relaxed);
true                                  // new generation — fire edge
```

The `poll_gen()` method was added to the `FileLike` trait with a default
return of 0 (meaning "not implemented, use level-triggered behavior").
Pipes override it to return their `state_gen`. Other file types (sockets,
eventfd, timerfd) can add generation tracking when needed.

Using `AtomicU64` for `last_gen` allows the lockfree `epoll_wait` fast
path (which accesses interests via `get_unchecked()` without locking) to
update the generation through `&self` without requiring `&mut`.

## Debugging approach

The initial plan hypothesized an interrupt masking issue (`cli` not
restored after `epoll_ctl`). Adding kernel `warn!` probes showed the
pipe read was reached, the lock was acquired, and the buffer had data
(`readable=true, free=4093`). But then — silence. No "slow path" message,
no return. The hang was inside the fast-path while loop, not in any
blocking sleep.

The lesson: with a non-empty buffer and a zero-length request, the
"obvious" code `while let Some(src) = pop_slice(remaining)` becomes an
infinite loop. The bug would never trigger without epoll because
`remaining_len()` is never 0 on the first iteration — only after reading
exactly the requested amount in a multi-pop loop.

## Results

```
Before: 77/86 PASS | 4 XFAIL | 0 DIVERGE | 5 FAIL
After:  79/86 PASS | 3 XFAIL | 0 DIVERGE | 4 FAIL
```

The two epoll tests moved from broken to passing. The known-divergences
list dropped from 4 to 3 entries (removed `events.epoll_level`).

## Files changed

| File | Change |
|---|---|
| `libs/kevlar_utils/ring_buffer.rs` | Fix `pop_slice(0)` infinite loop + wrapped-buffer wp/rp swap |
| `kernel/syscalls/mod.rs` | Pass `Option<UserVAddr>` for epoll_ctl event pointer |
| `kernel/syscalls/epoll.rs` | Accept `Option<UserVAddr>`, use `opened_files_no_irq()` |
| `kernel/fs/epoll.rs` | Use `lock_no_irq()` for interests, add EPOLLET + generation check |
| `kernel/pipe.rs` | Add `state_gen: AtomicU64` to PipeShared, increment on state changes |
| `libs/kevlar_vfs/src/inode.rs` | Add `poll_gen() -> u64` to FileLike trait |
| `testing/contracts/known-divergences.json` | Remove epoll_level entry |
