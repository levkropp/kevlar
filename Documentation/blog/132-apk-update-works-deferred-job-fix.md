# Blog 132: `apk update` works — deferred job fix unlocks TCP networking

**Date:** 2026-03-30
**Milestone:** M10 Alpine Linux — Phase 4 (Package Management)

## Summary

Alpine's `apk` package manager now works end-to-end on Kevlar. Root cause
of all TCP networking failures traced to a broken deferred job queue
(crossbeam SegQueue silently dropping items in bare-metal context) and
missing deferred job processing on the LAPIC timer path. With these fixes,
`apk update` and `apk add` both succeed.

## The TCP mystery

TCP connect had been hanging forever since the beginning of the project's
networking support. UDP (DNS) worked fine, TCP servers (dropbear SSH)
worked, but outbound TCP connect blocked indefinitely. Non-blocking
connect with poll() worked, but blocking connect did not.

### Root cause 1: crossbeam SegQueue broken in bare-metal

The deferred job queue used `crossbeam::queue::SegQueue` wrapped in a
`SpinLock`. Items were successfully `push()`ed (verified with logging)
but `pop()` never found them. The SegQueue implementation likely depends
on memory ordering or allocation behavior that doesn't work correctly in
our `#![no_std]` bare-metal environment.

**Fix:** Replaced `SegQueue<Box<dyn FnOnce()>>` with `Vec<Box<dyn FnOnce()>>`.
Simple, proven, no external dependencies.

### Root cause 2: LAPIC timer didn't process deferred jobs

The kernel has two timer interrupt paths:
- **PIT timer** (IRQ 0) → `handle_timer_irq()` → calls `run_deferred_jobs()`
- **LAPIC timer** (vector 0x40) → `handle_ap_preempt()` → did NOT call `run_deferred_jobs()`

After the LAPIC timer is initialized (which happens before userspace starts),
the PIT timer effectively stops being the primary preemption source. All
deferred jobs (including network packet processing) were orphaned.

**Fix:** Added `run_deferred_jobs()` call to `handle_ap_preempt()`.

### Root cause 3: Lock held during callback execution

The old `run_deferred_jobs()` held the `SpinLock` (which disables interrupts)
for the entire duration of all callbacks. This prevented network IRQs from
firing while `process_packets()` was running, creating a chicken-and-egg
problem for TCP handshake processing.

**Fix:** Release the lock before running each callback, re-acquire for the
next pop. This allows network IRQs to fire between callbacks.

## Packet processing flow (corrected)

```
Network IRQ fires
  → virtio_net handle_irq()
    → receive_ethernet_frame(packet)
      → RX_PACKET_QUEUE.push(packet)
      → PACKET_PROCESS_JOB.run_later(process_packets)
  → handle_irq() epilogue
    → run_deferred_jobs()
      → process_packets()
        → iface.poll() — delivers SYN-ACK to smoltcp TCP socket
        → SOCKET_WAIT_QUEUE.wake_all()
          → sleeping thread becomes Runnable

LAPIC timer fires
  → handle_ap_preempt()
    → run_deferred_jobs()  ← NEW: processes any queued jobs
    → switch() — picks up the now-Runnable thread
      → thread resumes in sleep_signalable_until
        → condition closure checks socket state → Established
        → connect() returns Ok(())
```

## apk update results

```
$ /sbin/apk update --no-check-certificate
fetch http://dl-cdn.alpinelinux.org/alpine/v3.21/main/x86_64/APKINDEX.tar.gz
fetch http://dl-cdn.alpinelinux.org/alpine/v3.21/community/x86_64/APKINDEX.tar.gz
v3.21.3-58-g...
OK: 25762 distinct packages available
```

`apk add file` also succeeds — downloads libmagic + file packages, installs
them to the ext4 root filesystem. Only a busybox trigger warning (non-fatal).

## Alpine boot status (cumulative)

| Component | Status |
|-----------|--------|
| OpenRC boot (14 services) | PASS (0 failures, 0 clock skew) |
| ext4 read/write + checksums | PASS (e2fsck clean) |
| DNS resolution (UDP) | PASS |
| TCP connect (apk, non-blocking) | PASS |
| `apk update` | PASS |
| `apk add <package>` | PASS |
| Clock tests (13/13) | PASS (Linux parity) |
| Shell prompt (getty) | PASS |

## Known remaining gaps

| Issue | Impact | Notes |
|-------|--------|-------|
| BusyBox wget (blocking TCP to remote) | Medium | apk works; wget uses different I/O model |
| DHCP auto-configuration | Low | Static IP in inittab works |
| `/proc/sys` fchdir | Low | sysctl removed from boot runlevel |

## Files changed

- `kernel/deferred_job.rs` — Replace SegQueue with Vec, release lock per callback
- `kernel/main.rs` — Add run_deferred_jobs to handle_ap_preempt
- `kernel/net/mod.rs` — Remove debug logging (clean up)
- `kernel/net/tcp_socket.rs` — Remove debug logging (clean up)
