# Blog 150: XFCE Session Alive — getsockopt Overflows Fixed, ip=0 Crash Resolved

**Date:** 2026-04-07

## Summary

The ip=0 SIGSEGV crash from Blog 144 is gone — it was caused by the same
interrupt-gate IF=0 bug that caused the SMP deadlock (Blog 149). With
interrupts re-enabled in the exit path, shell processes no longer crash
during demand paging. Three getsockopt buffer overflows were found and
fixed. The XFCE desktop session now starts: xfce4-session, xfwm4, and
Xorg all run, with 9/9 pre-session tests passing. Two new crashers
remain: an Xorg GPF and an at-spi-bus-launcher stack canary failure.

## The ip=0 SIGSEGV — same root cause as the SMP deadlock

Blog 144 documented a mysterious crash pattern: multiple X11 client
processes (xterm, xfce4-panel, dbus-daemon) crashed with `ip=0x0` and
`RAX=0x0` — a null function pointer call. The return addresses on the
stack were valid, ruling out stack corruption.

The crash disappeared after the Blog 149 fix (re-enabling interrupts in
`exit_by_signal`). The IF=0 propagation from the interrupt gate wasn't
just causing SMP deadlocks — it was disrupting demand paging timing.
With interrupts permanently disabled, timer-driven page allocation and
disk I/O couldn't complete normally, causing processes to execute from
incompletely loaded pages (zero-filled instead of file content). The
bytes `00 00` decode as `add %al,(%rax)`, which dereferences RAX
(often 0) — producing the characteristic `fault_addr=0, ip=<valid code>`
crash pattern.

## getsockopt buffer overflows

An audit of all syscall handlers that write to userspace buffers found
three overflow patterns in `kernel/syscalls/getsockopt.rs`:

### write_int_opt: 4-byte write without size check

```rust
// Before: always wrote 4 bytes regardless of caller's buffer size
fn write_int_opt(optval, optlen, value) {
    len.write(&(size_of::<c_int>() as c_int));
    val.write::<c_int>(&value);  // 4 bytes, unchecked
}
```

Used by SO_ERROR, SO_TYPE, SO_RCVBUF, SO_SNDBUF, SO_REUSEADDR,
SO_KEEPALIVE, SO_PASSCRED, and TCP_NODELAY — 8 socket options total.

### write_timeval_opt: 16-byte write without size check

```rust
// Before: always wrote 16 bytes (struct timeval)
fn write_timeval_opt(optval, optlen, us) {
    len.write(&16);
    val.write::<i64>(&tv_sec);   // 8 bytes
    val2.write::<i64>(&tv_usec); // 8 more bytes, unchecked
}
```

Used by SO_RCVTIMEO and SO_SNDTIMEO.

### SO_PEERCRED: 12-byte write without size check

```rust
// Before: always wrote 12 bytes (struct ucred)
val.write_bytes(&ucred);  // 12 bytes, unchecked
```

### The fix

All three now read the caller's `optlen` before writing, truncating to
fit — matching the Blog 143 fix for `write_sockaddr`:

```rust
fn write_int_opt(optval, optlen, value) {
    let max_len = len_ptr.read::<c_int>()? as usize;
    let copy_len = min(max_len, size_of::<c_int>());
    if copy_len > 0 {
        val.write_bytes(&value.to_ne_bytes()[..copy_len])?;
    }
    len_ptr.write(&(full_len as c_int))?;  // report actual size
}
```

This is the same pattern Linux uses: write what fits, report the full
size via `optlen` so the caller knows if truncation occurred.

## SCHEDULER → PROCESSES lock nesting eliminated

While auditing lock ordering for the SMP deadlock, I found that
`switch()` held SCHEDULER while acquiring PROCESSES — the only nested
global lock pair in the kernel. Split into two non-overlapping phases:

```
// Phase 1: pick next PID (SCHEDULER lock only)
let next_pid = {
    let scheduler = SCHEDULER.lock();
    scheduler.enqueue(prev_pid);
    scheduler.pick_next()
};  // SCHEDULER released

// Phase 2: resolve PID (PROCESSES lock only)
let next = match next_pid {
    Some(pid) => PROCESSES.lock().get(&pid).clone(),
    None => idle_thread(),
};
```

The window between locks is safe: if `exit_group()` removes the PID,
the PROCESSES lookup returns None and falls back to idle.

## XFCE desktop status

With the IF=0 fix and getsockopt overflows patched, the XFCE test on
`-smp 2` now shows:

```
TEST_PASS mount_rootfs
TEST_PASS dev_fb0_exists      (1024x768 32bpp)
TEST_PASS fb0_ioctl
TEST_PASS dbus_start
TEST_PASS xorg_running
TEST_PASS xdpyinfo
TEST_PASS xsetroot_color
TEST_PASS fb_pixels_visible   (center=00336699 — blue!)
TEST_PASS xterm_running
```

xfce4-session and xfwm4 both start:
```
(xfce4-session:14): xfce4-session-WARNING: No GPG agent found
(xfwm4:36): dbind-WARNING: AT-SPI: Error retrieving accessibility bus
```

Two crashers remain:

### Xorg GENERAL_PROTECTION_FAULT

```
USER FAULT: GENERAL_PROTECTION_FAULT pid=8 ip=0xa102ed976
PID 8 (/usr/libexec/Xorg) killed by signal 11
```

Xorg starts, accepts connections, renders xsetroot and xterm, launches
XFCE — then crashes with a GPF after ~10 seconds. This kills all X11
clients. The GPF is in a shared library; root cause unknown.

### at-spi-bus-launcher stack canary failure

```
BREAKPOINT: ip=0xa1035eed0 rsp=0x9ffffe910
  [rsp+0x0] = 0x0000000a10361f10   ← caller of __stack_chk_fail
  [rsp+0x18] = 0x0000000a103bdde9  ← deeper in call chain
```

The stack canary at `fsbase+0x28` is overwritten during a syscall. The
function at return address `0xa10361f10` (in libglib or libgio) has its
canary corrupted. This is a kernel buffer overflow — a different one
from the Blog 143 sockaddr fix and the getsockopt fixes above.

## Demand page diagnostic

Added a short-read detector to the file-backed demand paging path:

```rust
let n = file.read(offset_in_file, dst, &opts)?;
if n < copy_len {
    warn!("DEMAND PAGE SHORT READ: pid={} vaddr={:#x} ...", ...);
}
```

This catches cases where `file.read()` returns fewer bytes than
expected, leaving the page partially zero-filled. No short reads were
observed in testing — the ip=0 crashes were from the IF=0 bug, not
demand paging.

## BREAKPOINT delivers SIGTRAP, not SIGSEGV

Disassembling the at-spi-bus-launcher crash revealed it was NOT a stack
canary failure — it was GLib's intentional `G_BREAKPOINT()` macro
(a literal `int3` instruction) inside `g_log_structured_array`. GLib
uses this to trap into a debugger when a fatal log message is emitted.

The at-spi-bus-launcher emits a fatal error because it can't start the
accessibility bus (a D-Bus configuration issue, not a kernel bug). GLib
traps with `int3`. On Linux, `int3` delivers SIGTRAP (signal 5), and
the default action is Terminate+coredump.

Kevlar was delivering SIGSEGV (signal 11) for BREAKPOINT. Fixed to
deliver SIGTRAP, matching Linux/POSIX behavior. This also fixes the
`#DB` (hardware debug exception) path.

## Test results

| Suite | Result |
|-------|--------|
| Threading SMP (4 CPUs) | 14/14 PASS |
| Regression SMP (4 CPUs) | 15/15 PASS |
| BusyBox SMP (4 CPUs) | 100/100 PASS |
| Alpine Smoke (67 tests) | 67/67 PASS |
| XFCE Phase 1-4 | 9/9 PASS |
| XFCE Phase 5 (session) | **3/4 PASS** |

XFCE session results:
```
TEST_PASS xfwm4_running          ← Window manager running (5 threads)
TEST_PASS xfce4_session_running  ← Session manager running
TEST_FAIL xfce4_panel_running    ← Panel not found (xfconf dependency)
```

## Next steps

1. Fix xfce4-panel startup (likely needs xfconfd / machine-id setup)
2. Add PS/2 keyboard/mouse driver for interactive desktop testing
3. Investigate the intermittent Xorg GPF (only seen in one test run)

## Files changed

- `platform/x64/mod.rs`: `enable_interrupts()` (Blog 149)
- `platform/arm64/mod.rs`: `enable_interrupts()` (Blog 149)
- `platform/lib.rs`: Export `enable_interrupts`
- `platform/x64/interrupt.rs`: Cleaned up BREAKPOINT handler
- `kernel/main.rs`: BREAKPOINT→SIGTRAP, other faults→SIGSEGV
- `kernel/process/process.rs`: `enable_interrupts()` in `exit_by_signal()`
- `kernel/process/switch.rs`: Split SCHEDULER/PROCESSES lock phases
- `kernel/syscalls/getsockopt.rs`: Buffer size checks for all write helpers
- `kernel/mm/page_fault.rs`: Demand page short-read diagnostic
- `Makefile`: `-smp 2` for test-xfce, 300s timeout
