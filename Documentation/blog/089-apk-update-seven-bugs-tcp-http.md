# Blog 089: Nine bugs to `apk update` — from DNS silence to 100/100 BusyBox

**Date:** 2026-03-19
**Milestone:** M10 Alpine Linux

## The problem

After fixing the heap VMA corruption (blog 088), `apk update` successfully
resolved DNS but exited with code 1 within ~1 ms of printing "fetch
http://dl-cdn.alpinelinux.org/...". No error message, no unimplemented syscall
warnings. The TCP/HTTP fetch path was failing silently.

## Diagnosis approach

We captured syscall traces using ktrace with `ktrace-syscall` and `ktrace-net`
features, then decoded PID 6's timeline to follow the exact syscall sequence
between DNS resolution and exit. The investigation uncovered *seven* distinct
bugs in the network stack, timer subsystem, and syscall layer — all of which
needed fixing before `apk update` could complete.

## Bug 1: MonotonicClock::nanosecs() always returns current time

**Symptom:** poll() with a 2500 ms timeout blocks for 30 seconds (until
SIGTERM from BusyBox `timeout`).

**Root cause:** `MonotonicClock::nanosecs()` on x86_64 unconditionally called
`nanoseconds_since_boot()` via TSC, ignoring the `self.ticks` field that was
captured when the clock snapshot was created:

```rust
pub fn nanosecs(self) -> usize {
    #[cfg(target_arch = "x86_64")]
    if kevlar_platform::arch::tsc::is_calibrated() {
        return kevlar_platform::arch::tsc::nanoseconds_since_boot();
        // ^^^ always returns NOW, not the snapshot time!
    }
    self.ticks * 1_000_000_000 / TICK_HZ
}
```

This meant `elapsed_msecs()` computed `now - now ≈ 0`, so the timeout condition
`elapsed_msecs() >= timeout` was never true. Every poll/select/epoll timeout in
the entire kernel was broken.

**Fix:** Store the TSC nanosecond value at creation time in a new `ns_snapshot`
field, and return it from `nanosecs()` instead of re-reading TSC.

## Bug 2: UDP sendto uses source IP 0.0.0.0 before DHCP is processed

**Symptom:** The first DNS query goes out with source IP 0.0.0.0. The response
arrives addressed to 0.0.0.0:50000, which smoltcp drops because the socket was
rebound to 10.0.2.15:50000 by the second sendto.

**Root cause:** The sendto rebind logic checked `iface.ip_addrs()` to get the
real interface IP. But at the time of the first sendto, the DHCP Ack packet was
sitting in `RX_PACKET_QUEUE` unprocessed — process_packets() hadn't been called
yet. The interface still had 0.0.0.0, so the rebind was skipped. Then
process_packets() ran (to transmit the DNS query), which *also* processed the
DHCP Ack and set the IP to 10.0.2.15 — but the DNS query had already been
enqueued with source 0.0.0.0.

We confirmed this with frame-level packet logging:

```
rx udp: 10.0.2.3:53 -> 0.0.0.0:50000 len=145    ← dropped!
rx udp: 10.0.2.3:53 -> 10.0.2.15:50000 len=157   ← accepted
```

**Fix:** Call `process_packets()` at the start of sendto, before checking the
interface IP. This flushes any pending DHCP completion so the rebind sees the
real address.

## Bug 3: ARP pending packet silently dropped

**Symptom:** Two back-to-back DNS sendto calls result in only one DNS query
reaching the wire. The first query is silently dropped.

**Root cause:** smoltcp's neighbor cache stores at most one pending packet per
destination IP. When the first sendto triggers an ARP request (cold cache), the
DNS packet is stored as "pending" in the cache. The second sendto enqueues
another packet to the same destination — and smoltcp replaces the first pending
packet with the second.

Confirmed via ktrace NET_TX_PACKET events: ARP request (42 bytes) went out, but
only one DNS query (82 bytes) was transmitted after ARP resolved.

**Fix:** Detect ARP transmission via an `ARP_SENT` flag set in
`OurTxToken::consume()` when an EtherType 0x0806 frame is sent. After sendto's
process_packets(), if ARP was triggered, spin for up to 1 ms with interrupts
enabled, polling `RX_PACKET_QUEUE` for the ARP reply. Once the reply arrives,
call process_packets() again to flush the pending packet before returning.

## Bug 4: recvmsg doesn't populate msg_name (source address)

**Symptom:** musl's DNS resolver receives both A and AAAA responses (103 + 115
bytes) but ignores them. It retries, receives them again, and eventually times
out — giving up on DNS.

**Root cause:** musl implements `recvfrom()` as a wrapper around the `recvmsg`
syscall. Our `sys_recvmsg` called `file.recvfrom()` to get the data and source
address, but discarded the source address with `_src_addr`:

```rust
let (read_len, _src_addr) = file.recvfrom(buf, ...)?;
// ^^^ source address thrown away!
```

musl's DNS resolver checks `sa.sin.sin_port` in the returned sockaddr against
the nameserver's port (53). Since msg_name was never written, the port was 0,
and musl rejected every DNS response.

**Fix:** Write the source address to `msghdr.msg_name` using `write_sockaddr()`
after the first successful recvfrom.

## Bug 5: TCP RecvError::Finished sleeps forever

**Symptom:** After HTTP response is received and the server sends FIN, the
kernel's TCP read blocks forever instead of returning EOF.

**Root cause:** `RecvError::Finished` (remote closed connection) was handled
identically to `Ok(0)` (empty receive buffer):

```rust
Ok(0) | Err(tcp::RecvError::Finished) => {
    if options.nonblock { Err(EAGAIN) }
    else { Ok(None) }  // ← sleep forever on FIN!
}
```

**Fix:** Separate the two cases. `Ok(0)` sleeps (waiting for more data).
`RecvError::Finished` returns `Ok(Some(0))` — EOF.

## Bug 6: TCP poll doesn't report POLLIN for EOF

**Symptom:** Applications using poll/epoll to wait for readable data are never
notified when the remote end closes the connection.

**Fix:** Set `POLLIN` when `!socket.may_recv()` and the TCP state is
CloseWait, LastAck, TimeWait, or Closing.

## Bug 7: TCP write doesn't block when send buffer full

**Symptom:** Blocking TCP write returns 0 immediately when the send buffer is
full, instead of waiting for space.

**Fix:** When `send()` returns `Ok(0)` with nothing written yet in blocking
mode, sleep on `SOCKET_WAIT_QUEUE` until `can_send()` becomes true.

## Additional fixes

- **getsockopt SO_ERROR:** Improved to distinguish ECONNREFUSED (no POLLHUP)
  from ECONNRESET (with POLLHUP) instead of always returning 111.
- **ktrace-decode.py:** Added syscall names for sendmsg (46), recvmsg (47),
  and setsockopt (54).

## Bug 8: vDSO page leaked on every fork

**Symptom:** After ~130 fork+exec+wait cycles, child processes crash with
`GENERAL_PROTECTION_FAULT` or `SIGSEGV at 0xff`. Tests pass individually and
in 200-iteration loops, but fail in the full 100-test BusyBox suite.

**Root cause:** `alloc_process_page()` in `platform/x64/vdso.rs` allocates a
per-process vDSO data page (4 KB) during fork. This page was never freed —
`Process::drop()` didn't include deallocation. After 130 forks: 520 KB leaked.

**Fix:** Free the vDSO page in `Process::drop()`:
```rust
let vdso_paddr = self.vdso_data_paddr.load(Ordering::Relaxed);
if vdso_paddr != 0 {
    free_pages(PAddr::new(vdso_paddr as usize), 1);
}
```

## Bug 9: GC starvation under CPU-busy workloads

**Symptom:** Even with the vDSO fix, the BusyBox test suite (100 fork+exec
cycles back-to-back) still crashed after ~130 processes.

**Root cause:** `gc_exited_processes()` only ran when the idle thread was
active (`current_process().is_idle()`). During the test suite, the CPU was
100% busy — the idle thread never ran. Exited processes accumulated in
`EXITED_PROCESSES`, and their resources were never freed:

- Per process: 1 vDSO page (4 KB) + 4 kernel stack pages (16 KB) = 20 KB
- After 130 processes: **2.5 MB** of kernel stacks + 520 KB vDSO pages leaked
- Page allocator under pressure → returns corrupted/stale pages → GPF/SIGSEGV

**Fix:** Remove the `is_idle()` guard. Exited processes have already called
`switch()` to yield the CPU, so their kernel stacks are no longer on any CPU
and are safe to free from any context (timer IRQ, interrupt exit).

**Result:** BusyBox tests go from 97–98/100 to **100/100**.

## The debugging journey

The seven bugs formed a dependency chain — each one masked the next:

1. **MonotonicClock** → poll timeouts broken → DNS resolver hangs forever
2. **DHCP flush** → first DNS response addressed to 0.0.0.0 → dropped
3. **ARP pending** → first DNS query never transmitted → only one response
4. **msg_name** → DNS responses rejected by musl → DNS "succeeds" but resolver
   doesn't see matches → retries until timeout

Fixing 1–3 got DNS responses delivered. Fixing 4 let musl match them. At that
point DNS completed, TCP connected, and the HTTP fetch worked — but only because
fixes 5–7 were also in place to handle the TCP data path correctly.

The critical diagnostic tool was ktrace with frame-level packet inspection.
Adding source/destination IP:port logging to `receive_ethernet_frame()` instantly
revealed the 0.0.0.0 source IP bug that had been invisible in syscall-level
tracing.

## Result

```
fetch http://dl-cdn.alpinelinux.org/alpine/v3.21/main/x86_64/APKINDEX.tar.gz
DHCP: got a IPv4 address: 10.0.2.15/24
v3.21.6-64-gf251627a5bd [http://dl-cdn.alpinelinux.org/alpine/v3.21/main]
OK: 5548 distinct packages available
ktrace_apk: apk exited with code 0
```

`apk update` successfully fetches the Alpine package index over HTTP. This is
the first time Kevlar has completed a full DNS → TCP → HTTP → gzip pipeline
using an unmodified distro binary. BusyBox tests improved from 97/100 to
100/100 thanks to the resource leak fixes.

## Files changed

- `kernel/timer.rs` — MonotonicClock ns_snapshot for correct elapsed time
- `kernel/net/mod.rs` — ARP_SENT flag in OurTxToken for ARP detection
- `kernel/net/udp_socket.rs` — DHCP flush + ARP wait in sendto
- `kernel/net/tcp_socket.rs` — EOF on FIN, POLLIN for EOF, blocking write
- `kernel/syscalls/recvmsg.rs` — populate msg_name with source address
- `kernel/syscalls/getsockopt.rs` — distinguish ECONNREFUSED vs ECONNRESET
- `kernel/process/process.rs` — free vDSO page in Process::drop, eager GC
- `kernel/mm/vm.rs` — TODO: page table teardown (intermediate pages still leak)
- `tools/ktrace-decode.py` — added sendmsg/recvmsg/setsockopt names
