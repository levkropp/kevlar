# Blog 127: Phase 2 — socket options, SSH, critical syscall dispatch bug, 52 benchmarks

**Date:** 2026-03-29
**Milestone:** M10 Alpine Linux — Phase 2 (Network Services)

## Summary

Phase 2 delivers production-ready networking for Alpine compatibility:

1. **Socket option enforcement** — SO_REUSEADDR, SO_KEEPALIVE, TCP_NODELAY,
   SO_RCVTIMEO, SO_SNDTIMEO stored per-socket and enforced in read/write
2. **Critical bug fix** — SYS_SETRLIMIT in wrong cfg block caused a catch-all
   match arm that routed ALL unmatched syscalls through setrlimit → SIGSEGV
3. **SSH integration** — Dropbear keygen, startup, listen verified (3/3 pass)
4. **Loopback networking** — 127.0.0.1/8 support with TX loopback + ARP
5. **52 benchmarks** — 9 new Phase 1/2 benchmarks, 24 faster than Linux KVM

## Socket option enforcement

### Per-socket storage

Added option fields to `TcpSocket` and `UdpSocket`:

- `reuseaddr: AtomicCell<bool>` — skip INUSE_ENDPOINTS check in bind()
- `keepalive: AtomicCell<bool>` — calls smoltcp `set_keep_alive(75s)`
- `nodelay: AtomicCell<bool>` — calls smoltcp `set_nagle_enabled(false)`
- `rcvtimeo_us: AtomicCell<u64>` — timeout in TCP read(), UDP recvfrom()
- `sndtimeo_us: AtomicCell<u64>` — timeout in TCP write()

### Timeout implementation

Uses the established pattern from epoll_wait/rt_sigtimedwait: capture
`MonotonicClock` before the sleep loop, check `elapsed_msecs()` inside the
condition closure. Returns EAGAIN on timeout expiry.

```rust
let started_at = if timeout_us > 0 {
    Some(crate::timer::read_monotonic_clock())
} else { None };
SOCKET_WAIT_QUEUE.sleep_signalable_until(|| {
    if let Some(start) = started_at {
        if (start.elapsed_msecs() as u64) * 1000 >= timeout_us {
            return Err(Errno::EAGAIN.into());
        }
    }
    // ... normal recv logic
})
```

### setsockopt/getsockopt dispatch

Rewrote both syscall handlers from stubs to real fd-resolving dispatch.
Uses the double-deref downcast pattern (`(**file).as_any().downcast_ref::<TcpSocket>()`)
documented in project memory.

## Critical bug: SYS_SETRLIMIT in wrong cfg block

### The bug

`SYS_SETRLIMIT` (160) and `SYS_GETRLIMIT` (163) were accidentally
defined inside the **ARM64** `syscall_numbers` module instead of the
**x86_64** module. On x86_64, these constants didn't exist.

In Rust, a match arm with an undefined constant name becomes a **variable
binding** — a catch-all that matches **any** value. The arm
`SYS_SETRLIMIT => self.sys_setrlimit(a1, UserVAddr(a2))` matched every
unhandled syscall, routing it through `sys_setrlimit` which interpreted
`a2` (the second argument — whatever it was) as a buffer pointer.

### The impact

For `prlimit64(0, RLIMIT_CORE, NULL, &buf)`:
- `a2 = 4` (the resource number RLIMIT_CORE)
- `sys_setrlimit(0, UserVAddr(4))` tried to read from address 4
- But actually wrote to address 4 (the `sys_getrlimit` path was taken
  for the GET variant) → SIGSEGV in `usercopy1b`

This affected **all programs** using any syscall not explicitly matched
before the `SYS_SETRLIMIT` arm. Dropbear, dbclient, and likely many
other static musl binaries crashed on their first `prlimit64` call.
BusyBox worked because its early syscalls were all in earlier match arms.

### The investigation

1. Added `CURRENT_SYSCALL_NR` global to track the dispatching syscall
2. Enhanced SIGSEGV crash dump with register context
3. Added per-syscall logging for PID > 5
4. Discovered `prlimit64` warn! inside match arm never fired
5. Added warn! to `SYS_SETRLIMIT` arm — discovered it matched n=157
   (prctl), n=165 (mount), n=47 (recvmsg), etc.
6. Compiler warning confirmed: `unreachable pattern` on SYS_SETRLIMIT

### The fix

Move `SYS_SETRLIMIT=160` and `SYS_GETRLIMIT=163` to the x86_64
`syscall_numbers` module. Remove stale `SYS_GETRLIMIT=97` (old 16-bit
ABI) duplicate.

## SSH integration test

### Infrastructure

- `testing/test_ssh_dropbear.c` — automated test program
- `make test-ssh` — Makefile target (no Alpine disk needed)
- `dbclient` added to initramfs alongside dropbear/dropbearkey

### Results: 3/3 PASS

| Test | Result |
|------|--------|
| ECDSA host key generation (dropbearkey) | PASS |
| Dropbear daemon startup (port 22) | PASS |
| Listen socket in /proc/net/tcp | PASS |

### QEMU SLIRP limitation

Guest-to-self TCP connections don't work in QEMU user-mode networking
(SLIRP has no hairpin NAT). The SYN stays in SynSent forever because
the packet goes to QEMU's virtual NIC but is never routed back.

End-to-end SSH testing uses `make run-alpine-ssh` + `ssh -p 2222 root@localhost`
from the host via port forwarding.

## Loopback networking

Added 127.0.0.1/8 to smoltcp's interface address list and implemented
TX loopback in `OurTxToken::consume()`:

- **IPv4 loopback**: packets to 127.0.0.0/8 or the interface's own IP
  are injected back into `RX_PACKET_QUEUE` instead of the wire
- **ARP loopback**: ARP requests for loopback addresses are converted
  to ARP replies (opcode 1→2, swap sender/target) so smoltcp learns
  the MAC for self-resolution
- **MAC swap**: src/dst MAC swapped on looped-back frames so smoltcp
  accepts them as incoming traffic
- **Own-IP cache**: `OWN_IPV4` atomic updated by DHCP, static config,
  and netlink `RTM_NEWADDR` for fast loopback detection in TX path

## Benchmarks: 52 total, 24 faster than Linux

### New Phase 1/2 benchmarks (9)

| Benchmark | Linux KVM | Kevlar KVM | Ratio |
|-----------|----------|-----------|-------|
| statx | 428ns | 383ns | **0.90x** |
| getsid | 97ns | 86ns | **0.89x** |
| getrlimit | 126ns | 130ns | 1.03x |
| prlimit64 | 127ns | 140ns | 1.10x |
| setrlimit | 128ns | 119ns | **0.93x** |
| fcntl_lock | 434ns | 386ns | **0.89x** |
| flock | 311ns | 306ns | 0.98x |
| setsockopt | 144ns | 118ns | **0.82x** |
| getsockopt | 183ns | 126ns | **0.69x** |

### Highlights

- **getsockopt 31% faster** than Linux — minimal downcast + atomic load
- **socketpair 3.1x faster** — streamlined Unix socket creation
- **mmap_fault 9x faster** — 64-page fault-around + page cache
- **getdents64 2.7x faster** — optimized directory iteration
- **sched_yield 2.7x faster** — lightweight scheduler path

### Regressions (3, all pre-existing)

| Benchmark | Linux | Kevlar | Gap | Cause |
|-----------|-------|--------|-----|-------|
| readlink | 383ns | 431ns | +12% | Path resolution overhead |
| mprotect | 1107ns | 1353ns | +22% | Huge page support checks |
| fork_exit | 44.4µs | 51.8µs | +17% | Larger Process struct |

## Files changed

| Area | Files |
|------|-------|
| Socket options | `kernel/net/tcp_socket.rs`, `udp_socket.rs`, `kernel/syscalls/setsockopt.rs`, `getsockopt.rs` |
| Syscall dispatch | `kernel/syscalls/mod.rs` (SYS_SETRLIMIT fix + CURRENT_SYSCALL_NR) |
| Loopback | `kernel/net/mod.rs`, `kernel/net/netlink.rs` |
| SSH test | `testing/test_ssh_dropbear.c`, `Makefile`, `tools/build-initramfs.py` |
| Benchmarks | `benchmarks/bench.c`, `tools/bench-report.py` |
