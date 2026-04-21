## Blog 201: the 23 s-per-client timeout is a Unix-socket data corruption

**Date:** 2026-04-21

Blog 200 found that `xfce4-panel` doesn't appear in `test-xfce`'s
process scan until T+48 because `xfce4-session` waits ~23 s per
Failsafe client for an XSM registration that never arrives.  That
prose paragraph proposed "trace the ICE/XSM handshake" as the next
move.

This blog closes the loop on "what does the trace show" and finds
the honest answer: it's a Kevlar Unix-socket data corruption, not
anything specific to XSM.

## The probe

`testing/test_xfce_panel_probe.c` started dumping
`/proc/<pid>/environ` for xfce4-session and xfwm4 — which
immediately returned bytes that looked like *only*
`KEVLAR_PID=1\0`.  That was a false lead: `ProcPidEnviron::read`
was a stub returning a synthetic string so OpenRC's md5-based
liveness check sees unique content per pid.  The stub overrode the
real environ.  Fixed in commit `5cffe1a` to return
`Process.environ` (a new field, populated on execve and inherited
on fork).

With the real environ in place, `/proc/29/environ` for xfwm4 shows:

```
SESSION_MANAGER=local/kevlar:@/tmp/.ICE-unix/17,unix/kevlar:/tmp/.ICE-unix/17
DISPLAY=:0
HOME=/root
DBUS_SESSION_BUS_ADDRESS=unix:path=/tmp/.dbus-session-sock
...
```

So `SESSION_MANAGER` *is* inherited.  xfwm4 knows where to find the
session manager.  xfwm4's 7 open fds include the ICE socket
connection.  The `@` prefix on the socket path means xfce4-session
listened on an *abstract-namespace* AF_UNIX socket — `/tmp/.ICE-unix/`
is empty by design; the socket name is in the abstract Linux
namespace.

## The real failure

`/mnt/tmp/xfce-session.log` — which I hadn't been reading carefully
enough — has the actual error:

```
(xfce4-session:17): GLib-GIO-WARNING: _g_dbus_worker_do_read_cb:
  error determining bytes needed: Blob indicates that message
  exceeds maximum message length (128MiB)

(xfwm4:29): libxfce4ui-WARNING: ICE I/O Error
(xfwm4:29): xfwm4-WARNING: Failed to connect to session manager:
  Failed to connect to the session manager: IO error occurred
  doing Protocol Setup on connection
```

The 128 MiB message is the D-Bus socket path.  The ICE Protocol
Setup error is the XSM socket path.  Both are Unix-socket path,
both are reading a length or header field and getting garbage.

This is a classic signature of stream desync: a reader expects a
header at offset 0, but the socket delivered them data starting at
some offset within a *previous* message.  The four bytes they
interpret as "message length" are actually bytes 4-7 of the prior
message body.

## What it is not

- Not a lock ordering problem (no SPIN_CONTENTION / NMI
  WATCHDOG events during the failure).
- Not the recently-fixed TLB deadlock (blog 199 — that was
  munmap-specific and fired differently).
- Not a missing syscall (xfwm4 gets far enough to *call* sendmsg
  and recvmsg — they return normal-looking sizes, just with bad
  data).

## What it probably is

A data-integrity bug in the `UnixStream` read or write path in
`kernel/net/unix_socket.rs`.  Candidates:

- Ring-buffer write-then-read sequencing: when `writable_contiguous`
  returns a split slice (wp near CAP), the split + wrap path
  could leak bytes across the wrap point.  The ring buffer tests
  cover the basic case but may not exercise the exact
  sendmsg-iovec sequence ICE uses.
- Race on the tx lock during concurrent sendmsg from sibling
  threads of a multithreaded client.  xfwm4 and GLib are both
  threaded.  Our `SpinLock::lock` serialises writes, but a
  *write* from thread A could interleave with a *read* from
  thread B if the lock granularity is wrong.
- SCM_RIGHTS / sendmsg iovec accounting: `sys_sendmsg` sends
  each iovec as a separate `file.sendto`.  If a following iovec's
  sendto returns short, the total returned to user is short
  too — but if user code doesn't check, it thinks more got sent
  than actually did, and the stream is now one message ahead on
  the receiver.

The probe + `SLOW_SYSCALL` instrumentation landed this round
make the next investigation cheap: re-run with -smp 2 against
a minimal XSM-client reproducer (not full XFCE) and watch which
sendmsg/recvmsg call has the desync.

## What landed this round

| commit | change |
|---|---|
| `5cffe1a` | `/proc/<pid>/environ` returns real environ, not a stub |
| `f952ef1` | SLOW_SYSCALL warn + panel-probe /proc/*/environ + /fd dump |

Nothing that fixes the socket corruption yet — this was a
narrowing / ruling-out round.  The bug is now localized to
"Unix stream-socket data corruption that manifests during the
D-Bus / ICE multi-round handshake but not during the test-xfce
simple probes."

## Next

1. Write a minimal AF_UNIX reproducer that does one client +
   one server sending a sequence of length-prefixed messages.
   Run under -smp 2 with the same workload shape as ICE
   Protocol Setup (4-byte header, small payload, multiple
   round-trips).  If it reproduces, the Unix-socket code is
   conclusively at fault and we have an in-tree test.
2. Audit `kernel/net/unix_socket.rs` write path for the
   split-slice wrap bug.  Specifically: does `advance_write(n)`
   handle `n == CAP - wp` (fills to CAP exactly, wraps wp to 0)
   in conjunction with the ring-buffer `full` flag?
3. Audit sys_sendmsg iovec handling against Linux's
   `sock_sendmsg` — short writes must not desync the stream.

Only after (2) or (3) land does chasing 4/4 on test-xfce make
sense.  Panel is not the bottleneck; a socket-corruption in the
infrastructure is.
