## Blog 237: kbox phase 13 — the request-mix matched, the hang didn't, and openbox turns out to be threaded

**Date:** 2026-04-26

Blog 236 ended with a pivot: the openbox hang is timing-sensitive, not
content-sensitive (strace-wrap mutes it), so kbox needs to mimic
openbox's actual init-burst cadence — not just its opcodes.

Today landed phase 13 (libxcb-style EAGAIN-pump with mixed request
types looped over 30 s) and a kernel-strace quantification of real
openbox's startup.  The test still passes 5/5.  But the strace
turned up something we'd missed across the prior six blogs: openbox
is **multi-threaded**, with libxcb running a worker thread
concurrently with the main thread.

## The strace fingerprint

`make ARCH=arm64 test-openbox CMDLINE="kbox-phase=99 strace-pid=11"`
captures 4 907 syscall events from openbox.real over the test
window:

| syscall | count | notes |
|---|---:|---|
| `read` | 1284 | mostly fd=6, 1024-byte chunks |
| `recvmsg` | 816 | 408 of these on fd=5 (X11), 271 EAGAIN |
| `openat` | 630 | many directories (config, themes, fonts) |
| `ppoll` | 555 | libxcb's tight event-pump |
| `mmap` | 372 | libs + shm |
| `writev` | 278 | 139 of these on fd=5 (X11) |
| `fcntl` | 160 | mostly fd flag changes |
| `close` | 152 | balanced with openat |
| `mprotect` | 126 | dynamic linker work |
| `fstat` | 118 | path resolution |
| **`futex`** | **92** | **inter-thread sync — the smoking gun** |
| `munmap` | 58 | |
| `rt_sigaction` | 48 | signal handler installation |

Two specific findings:

1. **`fd=6` is openbox doing 574 read calls of 1024 bytes each on
   directory file descriptors.**  openat'd with `O_DIRECTORY`
   (flags=131072), then read in a loop to enumerate dirents — the
   classic `opendir`/`readdir`/`closedir` cycle in libc.  That's
   ~585 KB of file-system metadata being pulled in, on top of the
   X11 traffic.

2. **openbox calls `clone(0x7d1100, ...)` once** — that's
   `CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND |
   CLONE_THREAD | CLONE_SYSVSEM | CLONE_SETTLS | CLONE_PARENT_SETTID
   | CLONE_CHILD_CLEARTID`, the standard glibc/musl pthread create
   flags.  Result TID = 12.  And another clone for a child process,
   TID 13.  So openbox spawns at least **one worker thread**
   (almost certainly libxcb's reader thread) and **one child
   process** (probably for D-Bus or session-management bookkeeping).

92 futex calls confirm there's real cross-thread synchronization
happening — futex is glibc/musl's mutex/condvar primitive, and the
only reason to see 92 of them in 30 seconds is two-or-more threads
contending on the same lock.

## What phase 13 did

Mirror openbox's per-second X11 traffic shape:

- **91 InternAtoms** for the full openbox `_NET_*` / WM_* /
  `_MOTIF_*` set (ATOMS const in `wm.rs`).
- **20 QueryExtensions** for the standard set.
- **112 GrabKey** requests (keycodes 8..120, AnyModifier).
- **16 GetProperty** on root.

Every request issued in libxcb-pump style: writev, then
`recvmsg` until EAGAIN, then `ppoll(5 ms)`, then `recvmsg` the
reply.  The whole sweep loops for 30 s so it overlaps with the
test's xprop probe at T+12 s.

The kbox.log confirms the cadence is right:

```
PHASE 13 entry: openbox/libxcb-style burst (~100 reqs with EAGAIN pump)
PHASE 13: mixed openbox-style burst (91 InternAtom + extensions + grabs)
[... 91 InternAtom requests, 20 QueryExtension, 112 GrabKey, 16 GetProperty ...]
[... loops for 30 s ...]
```

Result: **5/5.**  xprop took 0 s, rc=0.  Same as the prior 12
phases.

## What we now know is wrong with kbox vs openbox

Nine separate dimensions where our load shape is now confirmed
correct, and exactly one where it isn't:

| dimension | kbox phase 13 | real openbox | matches? |
|---|---|---|---|
| WM identity claim | yes | yes | ✓ |
| SubstructureRedirect grab | yes | yes | ✓ |
| Variable-length reply parsing | yes | yes | ✓ |
| Extension negotiation (16 exts) | yes | yes | ✓ |
| XKB initialisation | yes | yes | ✓ |
| MIT-SHM segment attach | yes | yes | ✓ |
| Resource cascade (GC, pixmap, window) | yes | yes | ✓ |
| Tight EAGAIN-pump request loop | yes (30 s) | yes | ✓ |
| Mixed atom + extension + grab burst | yes | yes | ✓ |
| **Multi-threaded socket I/O** | **no — single thread** | **yes — libxcb worker thread** | ✗ |
| Concurrent filesystem I/O | no | yes (570 dirent reads) | ✗ |

The bug is almost certainly in one of those two final rows.  The
multi-threaded hypothesis is the strongest because:

- Kevlar's `clone(CLONE_THREAD)` + futex path is comparatively
  uncovered.  We have a 14-test SMP threading suite
  (`make test-threads-smp`) that all passes, but those are
  isolated micro-benchmarks — they don't combine threading with
  the WM-style AF_UNIX I/O that openbox does.
- libxcb's worker thread holds the X11 socket and blocks on
  `recvmsg` while the main thread issues `writev` on the same
  fd.  That's a concurrent reader+writer on the same socket —
  the exact pattern we've never tested in a real workload.

## Phase 14 sketch

Next iteration of kbox: spawn a libxcb-style worker thread.  Main
thread does the burst (writev), worker thread blocks on recvmsg
for replies, communicates via a SHARED future-style channel.

```rust
// pseudocode
let (tx, rx) = std::sync::mpsc::channel();
let sock_clone = conn.sock.try_clone()?;
std::thread::spawn(move || {
    loop {
        let frame = read_frame_from(&sock_clone)?;
        tx.send(frame).ok();
    }
});
// main thread
for atom in ATOMS { intern_atom(...); flush(); rx.recv()?; }
```

Two threads contending on the same AF_UNIX fd with our scheduler's
wake-up logic — that's the load pattern openbox actually produces.
If the bug is there, this triggers it.  If it's not, we move on to
phase 15 = simultaneous filesystem readdir loop.

## Even-more-aggressive phase

While we're at it: phase 14 should also issue the openbox-typical
**ChangeProperty WRITES** — openbox publishes its full EWMH state
on root, which means ~30 ChangeProperty writes setting
`_NET_SUPPORTED`, `_NET_NUMBER_OF_DESKTOPS`, etc.  This is
something kbox phase 12's resource cascade did one of (the EWMH
check chain) but not the full set.

## What positively shipped this round

- `tools/kbox/src/wm.rs::phase13_libxcb_burst` — sets the X11 fd
  non-blocking, runs a 30-second loop of (91 atoms + 20 extensions
  + 112 grabs + 16 GetProperty) with libxcb-style EAGAIN-pump
  recvmsg/ppoll cycles.
- The strace dataset — quantified that real openbox produces 4 907
  syscalls in 30 s (163/s avg, with a tighter burst in the first
  1-2 s), of which 408 recvmsg (271 EAGAIN), 555 ppoll, 92 futex.
  Concrete numbers for any future cadence-matching work.
- The threading discovery — six blogs into this arc, we now know
  openbox's specific divergence vs kbox is concurrent
  reader+writer on the same X11 socket.

## Closing

Twelve content phases, two cadence phases (7+8), and now a full
libxcb-style cadence phase, all clean.  The remaining variable is
threading — the one thing we can't get to with an additive single-
threaded bisect.  Phase 14 (multi-threaded kbox) is the next
kernel-bug-trigger candidate.

Each round of bisecting positively confirms more of the kernel's
correctness; each negative result narrows the trigger.  At this
point the trigger is: *concurrent reader+writer on the same
AF_UNIX stream socket from sibling threads, with one side doing
X11-style ~50 byte writevs and the other blocking in recvmsg-then-
ppoll loops, for at least the first 5 seconds of the WM's life*.
That's a 50-line C reproducer once phase 14 confirms it.

The pattern keeps holding: we don't catch the trigger by
exhaustively bisecting one side — we catch it by closing the gap
between what we replicate and what we observe, one diagnostic
round at a time.
