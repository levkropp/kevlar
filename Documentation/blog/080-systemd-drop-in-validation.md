# 080: M9.8 — Comprehensive Systemd Drop-In Validation

## Context

Kevlar's M9 achieved real systemd booting with a 4-check smoke test (`test-m9`:
20s timeout, 4 grep checks). That was enough to prove the concept, but not enough
to trust. M9.8 raises the bar to a comprehensive validation: `make test-systemd`
chains a 25-test synthetic init-sequence suite (single-CPU + SMP) with a real
systemd v245 boot, giving confident evidence that Kevlar is a genuine Linux kernel
replacement for systemd workloads.

## Kernel bug fixes (Phase 1)

### Stable boot_id

`/proc/sys/kernel/random/boot_id` was calling `rdrand_fill` on every read,
producing a different UUID each time. systemd reads boot_id multiple times during
startup and expects the same value. The fix was straightforward:

```rust
static BOOT_ID: spin::Once<[u8; 37]>
```

The UUID is generated once via `call_once` and returned on every subsequent read.

### rt_sigtimedwait real implementation

The previous stub just yielded the CPU and returned EAGAIN. systemd uses
`rt_sigtimedwait` to wait for SIGCHLD from supervised services — always getting
EAGAIN caused a tight busy-loop that burned through the boot timeout.

The new implementation has three paths:

- **Fast path:** Dequeue an already-pending signal matching the wait mask and
  return immediately.
- **Sleep path:** `POLL_WAIT_QUEUE.sleep_signalable_until` with a computed
  deadline. Wake on any signal, then check the mask.
- **Zero timeout:** Immediate EAGAIN (poll semantics, used by systemd for
  non-blocking signal checks).

### FIOCLEX/FIONCLEX ioctls

systemd uses `ioctl(fd, FIOCLEX)` to set `FD_CLOEXEC` on file descriptors
instead of the `fcntl(F_SETFD)` path. These ioctls (`0x5451`/`0x5450`) fell
through to the per-file `ioctl` handler, which returned ENOSYS. Added handling
in `ioctl.rs` before the file delegation point.

### osrelease check fix

`mini_systemd_v3.c` test 23 checked for the string `"4.0.0"` in the uname
release, but the kernel now reports `"6.19.8"` (updated in blog 076). Changed
the check to accept `"5."` or `"6."` prefixes.

## Missing syscall dispatch (Phase 2)

Five syscalls that systemd calls during its init sequence were missing from the
dispatch table entirely:

| Syscall | Number | Implementation |
|---|---|---|
| `clock_nanosleep` | 230 | Relative sleep + `TIMER_ABSTIME` mode |
| `clock_getres` | 229 | Reports 1ns resolution for all supported clocks |
| `timerfd_gettime` | 287 | Reads remaining time + interval from `TimerFd` |
| `setns` | 308 | ENOSYS stub (namespace entry, not needed until M8) |
| `epoll_pwait2` | 441 | ENOSYS stub (suppresses log spam from glibc probing) |

`clock_nanosleep` was the most impactful — systemd's `sd-event` loop uses it
for deadline-based sleeping. Without it, event loop timeouts silently failed.

## procfs additions (Phase 3)

systemd reads several `/proc/sys` tunables during early boot and adjusts its
behavior based on the values. Four were missing:

| Path | Value | Purpose |
|---|---|---|
| `/proc/sys/kernel/kptr_restrict` | `1` | Hides kernel pointer addresses |
| `/proc/sys/kernel/dmesg_restrict` | `0` | Allows unprivileged dmesg access |
| `/proc/sys/vm/overcommit_memory` | `0` | Heuristic overcommit (default) |
| `/proc/sys/vm/max_map_count` | `65530` | Maximum mmap regions per process |

All are read-only stubs returning Linux default values. systemd doesn't write
to them — it just reads them to decide whether to enable certain features.

## Discoveries during validation

Testing with the host's systemd v259 (harvested automatically when the v245
from-source build fails) exposed several deeper compatibility issues. All fixes
also benefit v245.

### vDSO clock_gettime fallback

The vDSO only handled `CLOCK_MONOTONIC` and returned `-ENOSYS` for everything
else. musl retries with a real syscall on vDSO failure, but glibc does not — it
treats the vDSO return value as final. systemd v259 called
`clock_gettime(CLOCK_BOOTTIME_ALARM)` via the vDSO and got `-ENOSYS`, then
asserted.

The fix was a one-line change in the vDSO machine code. Instead of:

```asm
mov eax, -38    ; -ENOSYS
ret
```

The fallback now does:

```asm
mov eax, 228    ; __NR_clock_gettime
syscall
ret
```

Unhandled clock IDs fall through to the real kernel syscall, which can return a
proper value or a proper error.

### Extended clock IDs

With the vDSO fallback fixed, the kernel syscall handler also needed to handle
the clock IDs that systemd actually uses:

| Clock ID | Value | Implementation |
|---|---|---|
| `CLOCK_PROCESS_CPUTIME_ID` | 2 | Returns monotonic time (approximation) |
| `CLOCK_THREAD_CPUTIME_ID` | 3 | Returns monotonic time (approximation) |
| `CLOCK_REALTIME_ALARM` | 8 | Aliases to CLOCK_REALTIME |
| `CLOCK_BOOTTIME_ALARM` | 9 | Aliases to CLOCK_BOOTTIME |
| `CLOCK_TAI` | 11 | Aliases to CLOCK_REALTIME (no leap offset) |

These were added to `clock_gettime`, `clock_getres`, and `clock_nanosleep`.

### TCGETS2 (modern glibc isatty)

Modern glibc (2.39+) uses `TCGETS2` (ioctl `0x802C542A`,
`_IOR('T', 0x2A, struct termios2)`) instead of the traditional `TCGETS`
(`0x5401`) for `isatty()`. The serial TTY and PTY devices only handled TCGETS,
so `isatty()` returned ENOSYS on modern glibc, causing systemd v259 to believe
it had no controlling terminal.

Added `TCGETS2`/`TCSETS2` handling to all three TTY types (serial, PTY master,
PTY slave).

### Default ioctl errno: EBADF to ENOTTY

The default `FileLike::ioctl()` returned `EBADF`, which is semantically wrong —
EBADF means "bad file descriptor" but the fd was perfectly valid. systemd v259's
`isatty_safe()` function has an assertion that EBADF should never come from a
valid fd. It did, and it crashed.

The correct POSIX return for "this fd doesn't support this ioctl" is `ENOTTY`
("inappropriate ioctl for device"). Changed the default in
`libs/kevlar_vfs/src/inode.rs`.

### New mount API stubs

systemd v259 requires `fsopen`/`fsconfig`/`fsmount` — the new mount API
introduced in Linux 5.2. Unlike v245, which uses the old `mount(2)` syscall and
works fine, v259 doesn't gracefully fall back.

Added ENOSYS stubs for six syscalls:

| Syscall | Number |
|---|---|
| `open_tree` | 428 |
| `move_mount` | 429 |
| `fsopen` | 430 |
| `fsconfig` | 431 |
| `fsmount` | 432 |
| `fspick` | 433 |

These stubs cause v259 to fail to mount, which is expected — full new-mount-API
support is tracked for a future milestone. v245 never calls them.

### Building systemd v245 from source

Building v245 on a modern host was its own adventure. Three issues:

1. **meson version:** v245 requires meson < 1.0. Installed 0.53.2 via pip.
2. **gperf:** Not packaged on the build host. Built from source into `~/.local`.
3. **GCC 15 compatibility:** `ARPHRD_MCTP` undefined (new in newer kernel
   headers), `-Werror` rejected new warnings. Patched both.

The `build-initramfs.py` script was updated to try the from-source build first
and fall back to harvesting the host's systemd binary plus all shared library
dependencies (discovered via `ldd`).

## Test infrastructure (Phase 4)

Three test targets, chained by `make test-systemd`:

| Target | What it does | Timeout |
|---|---|---|
| `test-systemd-v3` | 25-test synthetic init sequence, 1 CPU | 180s |
| `test-systemd-v3-smp` | Same 25 tests, 4 CPUs | 180s |
| `test-m9` | Real systemd v245 PID 1 boot, 4 grep checks | 90s |

The `test-m9` target was upgraded from 20s to 90s timeout and now prints
per-check PASS/FAIL status with a failed-unit count summary.

The synthetic suite (`mini_systemd_v3.c`) exercises the 25 syscall behaviors
that systemd's init sequence depends on most heavily — the same behaviors fixed
in Phases 1-3 above. Running it on both 1-CPU and 4-CPU configurations catches
any concurrency bugs in the new implementations (the `rt_sigtimedwait` sleep
path is particularly sensitive to SMP race conditions).

## Final results

```
$ make RELEASE=1 test-systemd
Step 1/3: synthetic init-sequence (1 CPU)      — 25/25 PASS
Step 2/3: synthetic init-sequence SMP (4 CPUs)  — 25/25 PASS
Step 3/3: real systemd PID 1 boot               — 4/4 PASS
  Welcome to Kevlar OS!
  systemd 245 running in system mode
  Reached target Kevlar Default Target.
  Started Kevlar Console Shell.
  Startup finished in 20ms (kernel) + 16ms (userspace) = 37ms.
=== M9.8 test-systemd: ALL PASSED ===
```

The 37ms boot time (20ms kernel + 16ms userspace) reflects Kevlar's syscall
performance advantage — systemd's init sequence is dominated by `clock_gettime`,
`epoll_wait`, and `rt_sigtimedwait`, all of which run faster on Kevlar than on
Linux KVM.

## Files changed

| File | Change |
|---|---|
| `kernel/fs/procfs/mod.rs` | Stable boot_id, kptr_restrict, dmesg_restrict, vm/ subdir |
| `kernel/syscalls/rt_sigtimedwait.rs` | New file: real implementation with fast/sleep/poll paths |
| `kernel/syscalls/mod.rs` | New dispatch entries, clock constants, syscall name table |
| `kernel/syscalls/ioctl.rs` | FIOCLEX/FIONCLEX handling |
| `kernel/syscalls/nanosleep.rs` | clock_nanosleep with relative + TIMER_ABSTIME modes |
| `kernel/syscalls/clock_gettime.rs` | clock_getres, extended clock IDs |
| `kernel/syscalls/timerfd.rs` | timerfd_gettime dispatch |
| `kernel/fs/timerfd.rs` | TimerFd::gettime() implementation |
| `kernel/fs/devfs/tty.rs` | TCGETS2/TCSETS2 handling |
| `kernel/tty/pty.rs` | TCGETS2/TCSETS2 for master + slave |
| `kernel/ctypes.rs` | New clock ID constants |
| `platform/x64/vdso.rs` | Syscall fallback instead of -ENOSYS return |
| `libs/kevlar_vfs/src/inode.rs` | Default ioctl returns ENOTTY instead of EBADF |
| `testing/mini_systemd_v3.c` | osrelease check accepts "5." and "6." |
| `tools/build-initramfs.py` | Host systemd harvesting, v245 from-source build |
| `Makefile` | test-systemd-v3-smp, test-m9 upgrade, test-systemd meta-target |

## What's next

M9.8 closes the systemd validation loop. The path forward is M10: Alpine Linux
text-mode boot. That means `/proc` completeness for musl's dynamic linker,
`/sys` for device enumeration, and enough of the block layer to mount a real
root filesystem. The contract test suite (112 tests) and systemd validation
(25 + 4 checks) form a regression safety net for everything that follows.
