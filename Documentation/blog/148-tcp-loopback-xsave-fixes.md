# Blog 148: TCP Loopback Fixed, xsave Intrinsic Corruption Found

**Date:** 2026-04-06

## TCP Loopback: Three Bugs, One Connection

TCP connections to 127.0.0.1 have never worked on Kevlar. The smoke test's
`p6_tcp_loopback` test hung indefinitely. Three bugs conspired:

### Bug 1: ARP Infinite Loop

When smoltcp needed the MAC address for 127.0.0.1, it sent an ARP request.
Our loopback code correctly intercepted it and crafted an ARP reply. But the
reply used **our own MAC** as the sender — and smoltcp silently ignores ARP
replies from its own MAC address (reasonable: why would you ARP yourself?).

Result: smoltcp sent ARP, got a reply it ignored, sent another ARP, got
another ignored reply... forever. The SYN packet was never transmitted.

**Fix:** Use a fake locally-administered MAC (`02:00:00:7f:00:01`) in the
loopback ARP reply. smoltcp accepts it, learns the neighbor entry, and
proceeds to send the SYN.

### Bug 2: POLLERR False Positive on Listening Sockets

Our `listen()` implementation creates backlog sockets (each in smoltcp's
LISTEN state) but never transitions the parent socket out of CLOSED. When
`poll()` checked the parent socket, it saw CLOSED and reported `POLLERR`.

The test's `poll(sfd, POLLIN, 5000)` returned immediately with `revents=8`
(POLLERR) instead of waiting for a connection. The application then called
`accept()`, which blocked forever since no connection existed.

**Fix:** Skip the POLLERR-on-CLOSED check for sockets that have backlogs
(i.e., listening sockets where CLOSED is the normal state).

### Bug 3: Loopback Frame Drain

`process_packets()` called `iface.poll()` in a loop until it returned
`PollResult::None`. But for loopback, each TX frame is injected back into
the RX queue via `OurTxToken::consume()`. After `iface.poll()` processed
one round of RX and generated TX responses, the TX-to-RX loopback frames
sat in the queue — and `iface.poll()` returned `None` because it didn't
see any new RX frames (it already drained the queue at the start of the
call).

**Fix:** After `PollResult::None`, check if `RX_PACKET_QUEUE` is non-empty.
If loopback frames are pending, do another round. This lets the full TCP
three-way handshake complete in a single `process_packets()` call.

## xsave Intrinsic Corruption

The Rust `_xsave64` and `_xrstor64` intrinsics from `core::arch::x86_64`
corrupt the kernel stack when compiled under the `x86-softfloat` target.
The corruption manifests as zeroed return addresses in the do_switch_thread
context frame, causing RIP=0 page faults after context switches.

The intrinsics likely generate SSE-using prologue/epilogue code that
clobbers the stack frame, despite the kernel being compiled with
`-sse,-sse2,...,+soft-float`. The soft-float target tells the compiler
not to use SSE for *computation*, but the intrinsic wrappers themselves
may use SSE for memory operations.

**Fix:** Replace the Rust intrinsics with inline assembly:

```rust
core::arch::asm!(
    "xsave64 [{}]",
    in(reg) ptr,
    in("eax") mask_lo,
    in("edx") mask_hi,
    options(nostack, preserves_flags),
);
```

The inline asm version generates a single `xsave64` instruction with no
compiler-inserted SSE code. Smoke test: 58/58 PASS (up from 56 before
loopback fix). XFCE session starts on single CPU.

## Phase Timing for Hang Detection

Added per-phase budget tracking to the smoke test. Each phase prints its
budget and actual elapsed time. If a phase takes more than 3x its budget,
a WARNING is printed. This makes hangs immediately visible instead of
waiting for the 10-minute Makefile timeout:

```
>>> Phase 6: Networking (budget 15s)
<<< Phase 6: Networking done (4s)
```

The Makefile timeout was also reduced from 600s to 180s.

## Status

| Test Suite | Result |
|-----------|--------|
| Smoke Phase 1-6 | 58/58 PASS |
| Smoke Phase 7-8 | Crash (page table corruption during fork) |
| XFCE (1 CPU) | xfce4-session runs, xfwm4 needs xfconf fix |
| XFCE (SMP) | Deadlock in concurrent process exit |

## Remaining: Phase 7-8 Crash

The smoke test crashes during Phase 7 (package management, which does
`apk update`) or Phase 8 (stress test with 50 rapid fork/exit). The crash
is a kernel page fault in `memcpy` called from `duplicate_table` during
`fork()`. The source pointer has corrupted upper bits (`7fff...` instead
of `ffff8000...`), suggesting page table entry corruption.

This is the next bug to investigate.
