# Milestone 4: systemd

**Goal:** Boot a minimal systemd as PID 1, reach multi-user.target, and run a
simple service.

**Current state:** Kevlar boots BusyBox + Bash with 103 syscalls.  systemd
requires ~30 additional syscalls centered on epoll event multiplexing, fd-based
event sources, filesystem mounting, and Unix domain socket enhancements.

## Phases

| Phase | Name | New Syscalls | Prerequisite |
|-------|------|-------------|--------------|
| [1](phase1-epoll.md) | Epoll | 3 | None |
| [2](phase2-event-fds.md) | Event Source FDs | 6 | Phase 1 |
| [3](phase3-unix-sockets.md) | Unix Sockets + D-Bus | 4 | Phase 1 |
| [4](phase4-mount.md) | Filesystem Mounting | 2 + /proc /sys | None |
| [5](phase5-process.md) | Process & Capabilities | 4 + UID/GID | None |
| [6](phase6-integration.md) | Integration Testing | stubs + fixes | All above |

Phases 1-3 are sequential (event fds and sockets depend on epoll).
Phases 4-5 are independent and can be done in parallel with 1-3.
Phase 6 is the final integration pass.

## systemd Boot Sequence (what we need to support)

```
1. PID 1 starts → sigprocmask, sigaction (HAVE)
2. epoll_create1 → main event fd (Phase 1)
3. signalfd4 → add to epoll (Phase 2)
4. timerfd_create → add to epoll (Phase 2)
5. mount /proc, /sys, /dev, /run (Phase 4)
6. open /dev/null, /dev/console (HAVE)
7. socket(AF_UNIX) → /run/systemd/notify (Phase 3)
8. Read unit files from /etc/systemd/ (HAVE - open/read/close)
9. epoll_wait → main loop (Phase 1)
10. Fork+exec services (HAVE - fork/execve)
```

## Architectural Constraints

- All new subsystems must respect the ringkernel architecture:
  - Epoll/event fd internals go in kernel core (Ring 1, safe Rust)
  - No new `unsafe` in the kernel crate
  - New pseudo-filesystems (/proc, /sys) remain kernel-coupled for now
- Each phase gets an integration test binary in `integration_tests/`
- Reference sources: FreeBSD `sys/compat/linux/` (BSD-2-Clause),
  POSIX/Linux man pages, Linux kernel documentation

## Key Design Decisions (to make during implementation)

1. **Epoll storage:** Vec-based interest list vs HashMap. HashMap is O(1)
   lookup for epoll_ctl(MOD/DEL) but heavier. Start with Vec, profile later.

2. **File description poll readiness:** Need a generic "is this fd readable/
   writable" trait that pipes, sockets, signalfd, timerfd, eventfd all
   implement. This is the `Pollable` trait — design it in Phase 1, use
   everywhere.

3. **Mount table:** Simple Vec of (source, mountpoint, fstype). No mount
   namespaces initially. procfs and sysfs are special-cased.

4. **Capabilities:** Start with stubs (all caps granted). Real capability
   checks come later when we have multi-user.

## Success Criteria

- `systemd --system` reaches `multi-user.target` without panic
- `systemctl status` shows running services
- A simple `.service` unit (e.g., a shell script) starts and stops correctly
- journald collects logs (or we stub it out initially)
