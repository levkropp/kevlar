# M9 Phase 4: Service Management — M9 Complete

Phase 4 validates the full systemd boot sequence end-to-end: service
startup, target reach, process visibility, and clean shutdown.

## Boot sequence

Under KVM, systemd v245 boots in ~200ms:

```
Welcome to Kevlar OS!
[  OK  ] Started Kevlar Console Shell.
[  OK  ] Reached target Kevlar Default Target.
Startup finished in 55ms (kernel) + 144ms (userspace) = 200ms.
```

## Phase 3.3 fixes (service lifecycle)

- **poll(timeout=0)**: returned 0 without checking fds because the
  timeout check ran before the fd poll loop. One-character fix
  (`> 0` → `>= 0`) unblocked systemd's entire event loop after fork.
- **procfs poll**: all procfs file types now return POLLIN so
  poll/epoll correctly reports them as readable.
- **/var/run symlink**: `/var/run -> /run` fixes systemd's "var-run-bad"
  taint warning.
- **/proc/sys/kernel/overflowuid, overflowgid, pid_max**: systemd reads
  these during manager initialization.

## Phase 4 verification

**ps aux** — BusyBox ps reads `/proc/[pid]/stat` and lists processes:
```
PID   USER     TIME  COMMAND
  1 root      0:00 sh -c ps aux
  2 root      0:00 ps aux
```

**Clean shutdown** — `reboot -f` triggers `reboot(LINUX_REBOOT_CMD_RESTART)`
which halts QEMU cleanly.

**Automated test** — `make test-m9` boots systemd under KVM and checks:
```
PASS: Started Kevlar Console Shell
PASS: Reached target Kevlar Default Target
PASS: Startup finished
PASS: Welcome banner
4/4 passed
```

## M9 summary

| Phase | Deliverable | Status |
|-------|-------------|--------|
| 1: Syscall gaps | waitid, memfd_create, flock, close_range, pidfd_open, mount flags | Done |
| 2: Init sequence | mini_systemd_v3 (25 tests), /proc/sys, /dev nodes, CLOCK_BOOTTIME | Done |
| 3.1: Build systemd | Prebuilt Ubuntu 20.04 systemd v245 in initramfs | Done |
| 3.2: Debug boot | Page fault double-fault, VMA split, permissive bitflags, /proc/self/fd deadlock | Done |
| 3.3: Service lifecycle | poll(timeout=0), procfs poll, event loop steady state | Done |
| 4: Services | make test-m9 (4/4), ps aux, clean reboot | Done |

systemd v245 runs on Kevlar as a drop-in Linux kernel replacement,
loading prebuilt Ubuntu binaries through the glibc dynamic linker.
