# Blog 102: Alpine Linux root login on Kevlar — OpenRC boots, shell works

**Date:** 2026-03-21
**Milestone:** M10 Alpine Linux

## Context

Blog 101 fixed the pipe crash (PIE relocation pre-faulting). This session
pushed through to a working Alpine login — fixing the remaining blockers
one by one with systematic tracing.

---

## Fix 1: Interpreter pre-fault (SIGSEGV at 0x19)

The blog 101 pre-fault fix only covered the main executable's writable data
pages. musl's interpreter also has a writable LOAD segment (`vaddr=0xa1aa0,
filesz=0x964`) that needs pre-faulting. Without it, fork children during
OpenRC service execution hit SIGSEGV at address 0x19 (another unpatched
relocation value).

Fix: refactored `prefault_writable_segments()` helper, called for both
main binary and interpreter ELF segments.

---

## Fix 2: Unix socket STREAM connect → ECONNREFUSED

**Root cause traced with syscall debug:**

```
socket(AF_UNIX, SOCK_STREAM) → fd 3
connect(3, "/var/run/nscd/socket") → 0     ← BUG: should be ECONNREFUSED
sendmsg(3, ...) → -107 ENOTCONN
```

musl's `initgroups()` tries to connect to nscd (name service cache daemon)
via a Unix socket. Our `connect()` returned success for non-existent listener
paths — even for `SOCK_STREAM` where POSIX requires `ECONNREFUSED`. The
stale `ENOTCONN` errno propagated through `initgroups → getgrouplist →
setgroups`, causing BusyBox login to report "can't set groups: Socket not
connected".

Fix: return `ECONNREFUSED` for `SOCK_STREAM` connect to non-existent
listeners. `SOCK_DGRAM` still returns success (systemd sd_notify pattern).

**Verified with test_login_flow.c:**
- `setgroups(0, NULL)` → 0 ✓
- `initgroups("root", 0)` → 0 ✓ (was -1/ENOTCONN)
- `getgrouplist("root", 0, ...)` → 12 groups ✓

---

## Fix 3: pivot_root syscall

Implemented real `pivot_root(new_root, put_old)`:
- Looks up filesystem mounted at `new_root`
- Makes its root directory the new root via `set_root()`
- Resets cwd to `/`
- Added `get_mount_at_dir()` to find mounted filesystems

This eliminates the `/mnt/root/` path prefix that broke OpenRC in blog 100.
OpenRC now starts cleanly without chroot path artifacts.

---

## Fix 4: make run-alpine target

Added `make run-alpine` Makefile target:
- First run builds ext4 image from Docker (`alpine:3.21` + openrc)
- Configures ttyS0 serial getty, empty root password
- Subsequent runs reuse cached `build/alpine.img`

---

## Alpine Boot Output

```
   OpenRC 0.55.1 is starting up Linux 6.19.8 (x86_64) [DOCKER]

 * /proc is already mounted
 * Mounting /run ...                                                [ ok ]
 * /run/openrc: creating directory
 * /run/openrc: correcting mode
 * /run/lock: creating directory
 * /run/lock: correcting mode
 * /run/lock: correcting owner
 * Caching service dependencies ...                                 [ ok ]

Welcome to Alpine Linux 3.21
Kernel 6.19.8 on an x86_64 (/dev/ttyS0)

kevlar login: root
Welcome to Alpine!

The Alpine Wiki contains a large amount of how-to guides and general
information about administrating Alpine systems.
See <https://wiki.alpinelinux.org/>.

login[31]: root login on 'ttyS0'
kevlar:~#
```

---

## Known Issues

| Issue | Severity | Notes |
|-------|----------|-------|
| 1 null pointer SIGSEGV (pid=21) during OpenRC boot | Low | Non-fatal, OpenRC recovers |
| `apk update` → "Error loading libz.so.1" | Medium | Library at /usr/lib/ not found by dynamic linker |
| `/dev/tty1-6` not found | None | Stock inittab, harmless |
| Clock skew warnings | None | No RTC, expected |

---

## Session Statistics

| Metric | Value |
|--------|-------|
| Commits this session | 15+ |
| Contract tests | 118/118 PASS |
| Benchmarks | 0 REGRESSION |
| ext4 integration | 30/30 PASS |
| Alpine boot | Login works |
| New syscalls | pivot_root |
| Bug fixes | PIE pre-fault (main+interp), ECONNREFUSED, SIGPIPE |
| Test programs written | 7 (pipe isolation) + 2 (login flow, Alpine shell) |
| Debug tooling | page_trace.rs (PTE walker, stack dumper) |
