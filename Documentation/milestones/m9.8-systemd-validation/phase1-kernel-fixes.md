# M9.8 Phase 1: Kernel Bug Fixes

## Overview

Four targeted kernel fixes that unblock the `mini_systemd_v3` test suite and
improve real systemd boot stability.

## 1.1 — Stable boot_id

**File:** `kernel/fs/procfs/mod.rs`

**Problem:** `ProcSysBootId::read()` calls `rdrand_fill` on every read, producing
a different UUID each time. systemd reads `/proc/sys/kernel/random/boot_id`
multiple times and expects a consistent value. The changing UUID causes
spurious mismatch warnings and breaks journald boot ID correlation.

**Fix:** Add a `static BOOT_ID: spin::Once<[u8; 37]>` at module level. The UUID
is generated once via `call_once` on first read; subsequent reads return the
cached value. `spin::Once` is already a dependency.

## 1.2 — rt_sigtimedwait Proper Implementation

**Files:** `kernel/syscalls/mod.rs` (replace stub), new `kernel/syscalls/rt_sigtimedwait.rs`

**Problem:** The current stub calls `switch()` once then returns EAGAIN. systemd
uses `rt_sigtimedwait` to wait for SIGCHLD from child services. Always getting
EAGAIN causes a busy-loop where services appear to never exit.

**Fix strategy:**
1. **Fast path:** If any signal in `set` is already pending, dequeue it,
   write `siginfo_t`, return signal number
2. **Sleep path:** Use `POLL_WAIT_QUEUE.sleep_signalable_until(deadline_fn)`,
   which is already wired to LAPIC timer IRQ
3. **Zero timeout:** Return EAGAIN immediately (poll semantics)

**Pattern references:**
- `kernel/syscalls/rt_sigsuspend.rs` — signal mask manipulation
- `kernel/process/signal.rs` — `pop_pending_masked` API
- `kernel/fs/epoll.rs` — POLL_WAIT_QUEUE usage

## 1.3 — FIOCLEX / FIONCLEX Ioctls

**File:** `kernel/syscalls/ioctl.rs`

**Problem:** systemd uses `ioctl(fd, FIOCLEX)` and `ioctl(fd, FIONCLEX)` to
set/clear `FD_CLOEXEC` on file descriptors. These ioctls (0x5451/0x5450)
currently fall through to the per-file `ioctl` handler, which returns ENOSYS.

**Fix:** Add handling before the `opened_file.ioctl(cmd, arg)` delegation:
```rust
const FIOCLEX: usize = 0x5451;
const FIONCLEX: usize = 0x5450;
if cmd == FIOCLEX || cmd == FIONCLEX {
    current_process().opened_files_no_irq().set_cloexec(fd, cmd == FIOCLEX)?;
    return Ok(0);
}
```

## 1.4 — Fix mini_systemd_v3 osrelease Check

**File:** `testing/mini_systemd_v3.c` line 328

**Problem:** Test 23 (`proc_sys_kernel`) checks that `/proc/sys/kernel/osrelease`
contains "4.0.0", but the kernel now returns "6.19.8".

**Fix:** Change the check to accept any modern kernel version:
```c
if (strstr(buf, "6.") == NULL && strstr(buf, "5.") == NULL) return 0;
```

## Verification

```bash
make check                             # type-check after each fix
make RELEASE=1 test-systemd-v3        # expect test 23 passes after 1.1 + 1.4
```
