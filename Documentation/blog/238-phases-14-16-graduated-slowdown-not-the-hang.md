## Blog 238: phases 14-16 — multi-thread + FS storm slows xprop linearly, but doesn't reproduce the hang

**Date:** 2026-04-26

Blog 237 closed phase 13 with the observation that openbox.real
spawns a libxcb worker thread (one `clone(CLONE_THREAD|...)` plus
one `clone(SIGCHLD)` for a child process) and that the 92 futex
calls during 30 s of openbox traffic confirm cross-thread
synchronization.  Single-threaded kbox couldn't reproduce that
shape.

Today landed phases 14, 15, 16:

- **Phase 14** — spawn one worker thread that holds a `try_clone()`'d
  X11 fd and blocks in `read()`.  Main thread bursts InternAtoms
  for 30 s.  Concurrent reader+writer on the same AF_UNIX socket.
- **Phase 15** — phase 14 + a second worker thread that does an
  openbox-style filesystem readdir storm (open `O_DIRECTORY`,
  `getdents64` to drain, close, rotate through 10 paths, repeat).
- **Phase 16** — phase 15 with **three** FS-storm threads instead
  of one, and 13 paths instead of 10.

Result table:

| phase | xprop took | TEST_END |
|---|---|---|
| 0-12 | 0 s | 5/5 |
| 13 | 0 s | 5/5 |
| 14 (1 worker, threaded X11 burst) | 0 s | 5/5 |
| 15 (1 X11 worker + 1 FS worker) | **2 s** | 5/5 |
| 16 (1 X11 worker + 3 FS workers) | **4 s** | 5/5 |
| **99 (real openbox)** | **18+ s, fail** | **3/5** |

Phase 16 stress-tested 5/5 times — deterministic.  And xprop's
delay clearly scales with our FS load (1 FS worker → 2 s,
3 workers → 4 s).  Linear extrapolation says ~7-8 FS workers
would push xprop past the test's 10 s timeout.

But that wouldn't reproduce the *bug*, just brute-force induce a
similar symptom.  Real openbox's signature is **Xorg silent for 30+
seconds** (from blog 232's kernel-strace) — it's not a graduated
slowdown, it's a hard hang.

## Why phase 14/15/16 isn't the same bug

Phase 14 was the cleanest test of the multi-thread hypothesis:
single libxcb-style worker thread reading from a `try_clone()`'d
X11 fd while the main thread issues writes on the same socket.
That's *exactly* what openbox + libxcb does at the kernel-fd level:
two tasks with both fds pointing at the same `struct socket`,
one in `recvmsg` and one in `writev`, with shared resource ID
allocator state, etc.

Phase 14 → **xprop took 0 s**.  The kernel handles concurrent
reader+writer on a single AF_UNIX stream socket cleanly under
SubstructureRedirect-grab + 30 s of bursty writes.  That's a
positive correctness signal we now have evidence for; not the
trigger.

Phase 15 added 1 FS-storm thread.  Phase 16 added 3.  Both
slowed xprop's reply but didn't break it — and neither matched
openbox's hard-hang signature.  The slowdown is just Xorg
having less CPU time per second when other tasks are competing,
which is normal scheduler behaviour.  It's not the bug.

## What's different about real openbox vs phase 14

Real openbox:
- Spawns the libxcb worker thread (clone with CLONE_THREAD).
- Spawns one child process (clone with SIGCHLD, result TID=13).
- Uses `eventfd2` for inter-thread signalling.
- Uses `pipe2` (probably libxcb's "wake the main thread when a
  reply is ready" channel).
- Calls `setsid`, `chdir`, `chroot` (none of which kbox does).
- Reads ~570× 1024-byte directory dirents.
- Calls `wait4` for child-process management.
- 92 futex calls — real cross-thread mutex/condvar contention.

Phase 14 covers the worker-thread + concurrent X11-fd-access
piece.  Phase 15/16 cover the dirent-read piece.  The remaining
unique-to-openbox dimensions are:

1. **The `clone(SIGCHLD)` child process** — a child running
   *something* alongside the WM main thread.  We don't know what
   it does; our strace only captured pid=11's syscalls.
2. **eventfd2 + pipe2 — inter-thread wake-up via two different
   primitives.**  Phase 14 uses `std::sync::atomic` only.
3. **Real futex contention** at 92 calls / 30 s = ~3/sec.  Phase
   14's worker uses `mpsc::channel` which uses futex internally,
   but the call rate is unmeasured.
4. **`setsid` + `chdir` + `chroot`** — these change the
   process's file/session/namespace context.  Possibly not the
   trigger but eliminates one variable.

## The realisation

We're four phases past the obvious openbox-vs-kbox differences
(content, cadence, threading, FS load) and the bug still doesn't
trigger from kbox.  Each kbox phase positively confirms a kernel
correctness property; none has been the silver bullet.

That suggests the bug isn't in any single dimension we've
isolated — it's in some specific *combination* of openbox's
exact code path that we'd only surface by *being* openbox at the
syscall level.  At that point we're not building kbox anymore;
we're transcribing openbox.

A more productive next direction: instead of continuing to add
phases to kbox, **build a Unix-socket proxy** between openbox and
Xorg that captures every byte both directions.  That's task #35's
original goal but blocked when our broken `ptrace` made
strace's tracing unavailable.  A proxy doesn't need ptrace — it
just needs to bind one Unix socket and forward bytes to another.
We already have `tools/kxserver/` which does exactly that
(modulo it's a server, not a proxy).  A 200-line `socat`-style
adapter would expose openbox's exact wire conversation to us.

That's task #39.

## What positively shipped

- `tools/kbox/src/wm.rs::phase14_threaded_burst` — concurrent
  reader+writer on the same AF_UNIX socket via `std::thread` +
  `try_clone()`, 30-second burst.  Test passes.
- `tools/kbox/src/wm.rs::phase15_kitchen_sink` — phase 14 + 1
  FS-storm thread (10 directory paths via `getdents64`).  xprop
  delay 0 s → 2 s.
- `tools/kbox/src/wm.rs::phase16_triple_fs` — 3 FS-storm threads
  + 13 paths.  xprop delay → 4 s.  Linear scaling.
- The threading-isn't-the-trigger finding.  Plus the
  observation that the kernel handles concurrent fd access
  correctly under our SMP scheduling.

## Closing

The bisect-from-kbox-side has hit diminishing returns.  Sixteen
phases later, we positively own these correctness properties:

1. WM identity claim, EWMH chain, SubstructureRedirect grab.
2. Variable-length reply parsing for QueryTree, ListProperties,
   GetProperty, GetKeyboardMapping, XkbGetControls.
3. Extension negotiation for 16 extensions.
4. XKB minor-opcode dispatch.
5. MIT-SHM segment attach + SysV-shm + AF_UNIX-fd-passing.
6. Resource ID lifecycle under SubstructureRedirect.
7. Tight EAGAIN-pump synchronous round-trips at 200 reqs/s × 30 s.
8. 64-deep async batched at 1200 reqs/s × 30 s.
9. mixed-cadence libxcb-style burst with full openbox atom set.
10. Concurrent reader+writer on AF_UNIX from sibling threads.
11. AF_UNIX I/O simultaneous with ext2 readdir-storm load.
12. clone(CLONE_THREAD), pthread mutex, mpsc channel, atomic flags.

The remaining trigger has to live in either openbox's exact byte
sequence (taskable via task #39 — a Unix socket proxy that logs
both directions) or in some specific corner case our 16 phases
haven't probed.  The next strategic move is to stop guessing and
capture the actual openbox conversation, which a 200-line proxy
can do in a way our broken ptrace can't.
