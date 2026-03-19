# M9.8 Phase 2: Missing Syscall Dispatch

## Overview

Add five syscalls to the dispatch table that systemd and glibc call during boot.
All additions are in `kernel/syscalls/mod.rs` (constants + dispatch match arms)
plus small handler functions.

## 2.1 — clock_nanosleep (syscall 230)

**File:** `kernel/syscalls/nanosleep.rs`

systemd and glibc use `clock_nanosleep` for high-resolution sleeps with
`CLOCK_MONOTONIC`. Currently returns "unimplemented syscall" log spam.

**Implementation:**
- `flags == 0` (relative): sleep for `tv_sec*1000 + tv_nsec/1_000_000` ms
- `flags == 1` (TIMER_ABSTIME): compute delta from `read_monotonic_clock()`,
  sleep the remainder
- Delegate to existing `_sleep_ms()` helper

## 2.2 — clock_getres (syscall 229)

**File:** `kernel/syscalls/clock_gettime.rs`

glibc calls `clock_getres` to determine timer resolution. Currently returns
ENOSYS, causing glibc to fall back to less efficient time APIs.

**Implementation:**
- If result pointer non-null: write `Timespec { tv_sec: 0, tv_nsec: 1 }`
  (1 nanosecond resolution — matches our TSC-based clock)
- Valid for all clock IDs supported by `clock_gettime`; EINVAL for unknown clocks

## 2.3 — timerfd_gettime (syscall 287)

**Files:** `kernel/syscalls/timerfd.rs`, `kernel/fs/timerfd.rs`

systemd calls `timerfd_gettime` to check remaining time on armed timers.
Currently returns ENOSYS.

**Implementation:**
- Add `TimerFd::gettime() -> (u64, u64)` returning `(remaining_ns, interval_ns)`
- Syscall handler reads the TimerFd, calls `gettime()`, writes a 32-byte
  `struct itimerspec` to the user pointer

## 2.4 — setns and epoll_pwait2 Explicit ENOSYS Stubs

systemd probes for these syscalls at boot and handles ENOSYS gracefully, but
the "unimplemented syscall" warnings add noise to the boot log.

```rust
pub const SYS_SETNS:        usize = 308;
pub const SYS_EPOLL_PWAIT2: usize = 444;
// In dispatch:
SYS_SETNS        => Err(Error::new(Errno::ENOSYS)),
SYS_EPOLL_PWAIT2 => Err(Error::new(Errno::ENOSYS)),
```

## Verification

```bash
make check                             # type-check all additions
make RELEASE=1 test-systemd-v3        # should eliminate "unimplemented" warnings
```
