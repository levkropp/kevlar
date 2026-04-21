## Blog 202: writev and sendmsg atomicity — the XFCE socket corruption, fixed

**Date:** 2026-04-21

Blog 201 narrowed the xfce4-panel flake to a Unix-socket data
corruption that manifested as `"Blob indicates that message exceeds
maximum message length (128MiB)"` in xfce4-session's D-Bus
connection and `"ICE I/O Error ... Protocol Setup on connection"`
in xfwm4's XSM registration.  The first reproducer tried simple
byte-at-a-time streaming and writev-of-iovecs from a single
writer — both passed cleanly, suggesting the bug was something
more specific.

## The reproducer that caught it

`testing/test_unix_stream_corruption.c` scenario 3, run under
`-smp 4`:

- `socketpair(AF_UNIX, SOCK_STREAM)` between thread pool and
  main thread.
- **Four** writer threads share the same send fd.
- Each writer does `writev(fd, 4-iovec, 4)` 20,000 times.
- Every iovec is a 4-byte word of a 16-byte framed message:
  `[magic=0xAA55AA55][len=16][seq=i][payload=(i<<8)|writer_id]`.
- Reader reads 16 bytes at a time, verifies magic, len, and
  that `seq == payload >> 8`.

First run:

```
FAIL concurrent frame 0: magic=aa55aa55 len=2857740885 seq=16 payload=00000000
```

`2857740885 = 0xAA55AA55` — the magic from a *different* writer's
next frame.  The reader saw `[magic_A][magic_B][len_B][seq_A]`,
which means writer A's first iovec (the magic) completed, then
writer B's entire 4-iovec writev ran, and then writer A's
remaining three iovecs.  The 4-iovec writev was **not atomic**.

## Root cause

`sys_writev` in `kernel/syscalls/writev.rs` and `sys_sendmsg` in
`kernel/syscalls/sendmsg.rs` both looked like this:

```rust
for iov in iovs {
    let written = opened_file.write(UserBuffer::from_uaddr(iov.base, iov.len))?;
    total += written;
}
```

Each `opened_file.write` takes the underlying file's lock
(`UnixStream.tx`), writes, releases.  Between iovecs, the lock
is free and another thread's writev can slip in.

Linux solves this by gathering all iovecs into a single
`sock_sendmsg` that walks the iovec vector inside one lock
acquisition.  For AF_UNIX stream sockets, the "atomic up to
SO_SNDBUF or some sensible limit" semantic is what D-Bus, ICE,
and several other length-prefixed wire protocols rely on.

## The fix (commit `6d950ce`)

In both `sys_writev` and `sys_sendmsg`, consolidate all iovecs
into a single kernel buffer (bounded to 64 KiB) and issue **one**
`write` / `sendto` call:

```rust
if total_len <= 64 * 1024 {
    let mut buf = vec![0u8; total_len];
    let mut off = 0;
    for iov in &iovs {
        iov.base.read_bytes(&mut buf[off..off + iov.len])?;
        off += iov.len;
    }
    return opened_file.write(UserBuffer::from(buf.as_slice()));
}
// Fallback per-iovec (non-atomic) for >64 KiB writes.
```

64 KiB matches Linux's default SO_SNDBUF and is large enough to
cover D-Bus messages, ICE handshake, HTTP headers, X11 requests,
and similar wire-format workloads.  Larger writev calls fall back
to the old per-iovec loop — at that size the atomicity guarantee
isn't reasonably expected (Linux also doesn't guarantee it), and
consolidating would use unbounded kernel heap.

## Verification

Reproducer, with fix:

```
OK concurrent: counts=[20000,20000,20000,20000] expected 20000 each
PASS — no stream corruption observed
```

4/4 consecutive runs clean.

XFCE startup, with fix:

- xfce-session.log no longer shows `"Blob indicates..."` or
  `"ICE I/O Error"`.  Only the benign `"No GPG agent"` and
  `"No SSH authentication agent"` warnings remain.
- `xfce4-panel` spawn time dropped from **T+48** to
  **T+13–T+24** across sampled runs.  The 23-s-per-client
  xfce4-session timeout from blog 200 was entirely the XSM
  handshake corrupting and restarting.

## What still remains

- `test-xfce`'s 15-s Phase-5 wait may still miss panel
  (especially on the T+24 side of the distribution).  That's a
  timing issue, not a correctness one — panel now *does* appear,
  just not always within 15 s.
- A separate SMP page-corruption class (blog 186 / task #25) can
  still fire during heavy XFCE startup, producing symptoms like
  INVALID_OPCODE in xfce4-session when a code-page byte gets
  flipped.  That one's unrelated to writev; see
  `project_xfce_page_corruption.md` for the candidate list of
  TLB-flush sites still carrying the deadlock pattern.
- The four remaining call sites of `flush_tlb_remote` while
  holding the Vm lock (mprotect, mremap, madvise, vm.rs) still
  need the blog-199 transform applied one-at-a-time.

## Commits landing this round

| commit | change |
|---|---|
| `9ad031b` | reproducer: `testing/test_unix_stream_corruption.c` |
| `6d950ce` | writev/sendmsg: atomic iovec consolidation ≤64KB |

Two commits.  One fix.  The improvement on XFCE is the largest
single-commit delta in the whole investigation arc: the 23-s
per-Failsafe-client XSM timeout is simply gone.
