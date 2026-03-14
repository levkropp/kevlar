# Phase 2: Systemd-Compatible Init Sequence

**Duration:** 4-5 days
**Prerequisite:** Phase 1
**Goal:** Build mini_systemd_v3, the comprehensive init-sequence test that exercises every codepath real systemd PID 1 touches during boot. This is the gating test — if it passes, the kernel is ready.

## mini_systemd_v3.c — 25 Subtests

Tests in systemd's actual boot order:

```
 1. set_child_subreaper    — prctl(PR_SET_CHILD_SUBREAPER)
 2. mount_proc_sys_dev     — mount /proc, /sys, /dev, /run, /dev/shm
 3. bind_mount_console     — mount --bind /dev/console /dev/console
 4. remount_nosuid         — mount -o remount,nosuid /dev
 5. tmpfs_run_systemd      — mkdir + mount tmpfs on /run/systemd
 6. set_hostname           — sethostname("kevlar")
 7. mount_cgroup2          — mount cgroup2 on /sys/fs/cgroup
 8. cgroup_hierarchy       — create init.scope, system.slice, user.slice
 9. move_pid1_cgroup       — move PID 1 into init.scope
10. enable_controllers     — +pids +cpu +memory in subtree_control
11. notify_socket          — AF_UNIX SOCK_DGRAM bind /run/systemd/notify
12. private_socket         — AF_UNIX SOCK_STREAM bind /run/systemd/private
13. main_event_loop        — epoll + signalfd + timerfd integration
14. fork_service           — fork child into system.slice/test.service cgroup
15. waitid_reap            — waitid(P_ALL, WEXITED|WNOHANG)
16. sd_notify_ready        — child sends "READY=1\n" on notify socket
17. memfd_data_pass        — memfd_create + write sealed data
18. pidfd_monitor          — pidfd_open + poll for process exit
19. close_range_exec       — close_range before exec
20. flock_lockfile         — flock on /run/systemd/private.lock
21. inotify_watch          — inotify on /run/systemd/system
22. service_restart        — child exits, parent detects via signalfd, re-forks
23. shutdown_sequence      — SIGTERM to children, wait, SIGKILL
24. read_proc_cgroup       — /proc/1/cgroup shows /init.scope
25. clock_boottime         — clock_gettime(CLOCK_BOOTTIME) returns valid time
```

## New Kernel Features Required

### AF_UNIX SOCK_DGRAM with bind()

systemd's sd_notify uses `socket(AF_UNIX, SOCK_DGRAM, 0)` + `bind("/run/systemd/notify")` + children `sendmsg()` to it.

**Files:** `kernel/net/unix_socket.rs` — verify/extend SOCK_DGRAM + bind() on filesystem path.

### CLOCK_BOOTTIME

systemd uses `CLOCK_BOOTTIME` (7) extensively. Add as alias for `CLOCK_MONOTONIC`.

**Files:** `kernel/syscalls/clock_gettime.rs` — add `CLOCK_BOOTTIME = 7` case.

### /proc/sys/ Entries

systemd reads these during boot:

| Path | Content |
|------|---------|
| `/proc/sys/kernel/hostname` | UTS hostname (writable) |
| `/proc/sys/kernel/osrelease` | `"4.0.0\n"` |
| `/proc/sys/kernel/random/boot_id` | Random UUID |
| `/proc/sys/fs/nr_open` | `"1048576\n"` |

**Files:** `kernel/fs/procfs/mod.rs` — add `sys` subdirectory tree with dynamic files.

### /dev Additions

| Device | Behavior |
|--------|----------|
| `/dev/kmsg` | Write = serial output, read = empty |
| `/dev/urandom` | Alias for /dev/random |
| `/dev/full` | Read = zeros, write = ENOSPC |

**Files:** `kernel/fs/devfs/mod.rs` — add device file implementations.

### /proc/[pid]/environ

systemd reads `/proc/1/environ` for its own environment.

**Files:** `kernel/fs/procfs/proc_self.rs` — add `"environ"` entry.

## Build Infrastructure

- `testing/Dockerfile` — add mini_systemd_v3 build stage
- `Makefile` — add `test-m9-init-seq` target

## Success Criteria

- [ ] mini_systemd_v3 passes 25/25 subtests
- [ ] AF_UNIX SOCK_DGRAM with bind() works for sd_notify
- [ ] CLOCK_BOOTTIME returns valid monotonic time
- [ ] /proc/sys/kernel/ entries readable
- [ ] /dev/kmsg writable
- [ ] All existing tests pass (no regression)
