# Phase 3.2: First Boot — systemd Banner

**Duration:** 3-4 days (iterative)
**Prerequisite:** Phase 3.1 (systemd binary in initramfs)
**Goal:** Boot systemd as PID 1 and reach `sysinit.target`. Fix the first ~10 kernel issues discovered during boot.

## Approach

1. Boot QEMU with `init=/usr/lib/systemd/systemd`
2. Capture serial output
3. Identify first failure (unimplemented syscall, missing file, wrong behavior)
4. Fix in kernel
5. Rebuild and retry
6. Repeat until systemd prints its banner

## Expected Failure Points (in boot order)

| # | syscall/file | Issue | Fix |
|---|-------------|-------|-----|
| 1 | `personality(0xffffffff)` | Not implemented | Stub: return PER_LINUX (0) |
| 2 | `mount(NULL, "/", NULL, MS_REC\|MS_PRIVATE, NULL)` | NULL source for "/" | Already handled (Phase 2) |
| 3 | `/dev/kmsg` write | systemd logs here early | Already implemented (Phase 2) |
| 4 | `seccomp()` calls | Process sandboxing | Return ENOSYS (systemd logs warning, continues) |
| 5 | `/proc/sys/kernel/random/boot_id` | Machine identity | Already implemented (Phase 2) |
| 6 | `CLOCK_BOOTTIME` | Timer operations | Already implemented (Phase 2) |
| 7 | `socket(AF_NETLINK, ...)` | udev event socket | Return EAFNOSUPPORT (systemd disables udev) |
| 8 | `/proc/1/environ` | Environment probe | Already implemented (Phase 2) |
| 9 | D-Bus connect | systemctl bus | Fail gracefully (degraded mode) |
| 10 | `ioctl(TIOCGWINSZ)` | Terminal size | Return 80x24 default |
| 11 | `/proc/1/comm` write | Set process name | May need writable /proc/[pid]/comm |
| 12 | `prctl(PR_SET_PDEATHSIG)` | Child death signal | Already returns 0 (stub) |

## New Syscalls/Stubs Expected

| Syscall | x86_64 | Behavior |
|---------|--------|----------|
| personality(135) | 135 | Return 0 (PER_LINUX) |
| seccomp(317) | 317 | Return ENOSYS |
| pselect6(270) | 270 | Map to existing poll/select |
| gettid(186) | already done | |
| getdents64(217) | already done | |

## /proc and /sys Additions

Files systemd probes that we may need to add:
- `/proc/cmdline` — already done
- `/proc/1/cmdline` — already done
- `/proc/1/cgroup` — already done (M8)
- `/proc/self/mountinfo` — already done (M8)
- `/proc/1/comm` — already done (read), may need write support

## Kernel Files to Modify (iteratively)

- `kernel/syscalls/mod.rs` — add personality, seccomp stubs
- `kernel/syscalls/ioctl.rs` — add TIOCGWINSZ default
- `kernel/net/` — return EAFNOSUPPORT for AF_NETLINK
- Various fixes discovered during boot attempts

## Testing

No formal test for this phase — it's an iterative debug cycle.
Success is measured by serial output: systemd's banner appears.

## Success Criteria

- [ ] systemd prints `systemd[1]: Detected architecture x86-64`
- [ ] systemd prints target progress messages
- [ ] systemd reaches `sysinit.target`
- [ ] No kernel panics during boot
- [ ] All existing tests still pass
