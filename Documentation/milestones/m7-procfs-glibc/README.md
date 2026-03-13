# Milestone 7: glibc Compatibility & /proc Filesystem

**Goal:** Run glibc-linked binaries and provide basic system introspection via
/proc. Enable distro programs (ps, top, htop, strace, etc.) and glibc-linked
binaries to work on Kevlar.

**Current state:** Kevlar runs musl-linked static binaries flawlessly (14/14
pthreads tests on 4 CPUs). glibc-linked binaries fail early in initialization
(futex operations, rseq, scheduling syscalls). No /proc filesystem exists —
even reading `/proc/self/maps` fails with ENOENT.

**Impact:** glibc is the libc for ~95% of distro binaries (system tools,
daemons, libraries). /proc is read at startup by nearly every process. Together
they unblock running real systemd, Python, Node.js, etc.

## Phases

| Phase | Name | Key Changes | Prerequisite |
|-------|------|------------|--------------|
| [1](phase1-procfs.md) | /proc Filesystem | VFS mount, inode implementation, key /proc files | None |
| [2](phase2-procfs-files.md) | /proc File Coverage | maps, status, cmdline, fd/, stat, statm, cpuinfo, meminfo | Phase 1 |
| [3](phase3-glibc-compat.md) | glibc Compatibility | futex ops, rseq stub, sched_setaffinity stub, signal mask compat | Phase 1 |
| [4](phase4-integration.md) | Integration Testing | glibc hello world, glibc pthreads, distro tools (ps, top, strace) | Phase 2 + 3 |

## Why /proc + glibc Together?

glibc and /proc are tightly coupled:
- glibc's malloc reads `/proc/self/maps` to understand memory layout
- Python's ctypes reads `/proc/self/maps` to find loaded libraries
- systemd reads `/proc/*/stat` to track processes
- `ps`, `top`, `htop` are completely /proc-dependent

Implementing them in parallel maximizes testing coverage and real-world
compatibility gains.

## Architectural Impact

### /proc Filesystem (Medium Impact)

New VFS service crate: `kevlar_procfs` (read-only, no writeable files initially).

- **Inode model:** Dynamic inodes (no pre-allocated dentries). `/proc/[pid]/` is
  a magic directory that enumerates live processes. `/proc/[pid]/maps` is
  computed on-read from the VMA tree. `/proc/[pid]/fd/` enumerates open FDs.
- **Permission model:** `/proc/[pid]/` files are readable only by the file
  owner (uid) or root. `/proc/cpuinfo`, `/proc/meminfo` world-readable.
- **Magic symlinks:** `/proc/self` → `/proc/[current_pid]`, `/proc/self/exe` →
  path to executable (new kernel state needed: `exe_path` in Process struct).

### glibc Compatibility (Medium Impact)

- **futex ops:** Add `FUTEX_CMP_REQUEUE`, `FUTEX_WAKE_OP`, `FUTEX_WAIT_BITSET`
  (used by condvars and robust mutexes). These are more complex than WAIT/WAKE
  but necessary for glibc NPTL.
- **rseq syscall (334):** Return ENOSYS cleanly. glibc 2.35-2.37 handles this,
  but we need to ensure errno is set correctly.
- **sched_setaffinity (203) / sched_getscheduler (145) / sched_setscheduler (144):**
  Stub as no-ops (Linux allows these to fail silently for processes without
  CAP_SYS_ADMIN).
- **signal mask compatibility:** Ensure sigprocmask stores match what
  /proc/[pid]/status reads.

## Test Plan

**Acceptance criteria:**
- `/proc/self/maps` is readable and shows correct VMA layout
- `cat /proc/cpuinfo` shows N CPUs (correct for `-smp N`)
- `ps` lists processes and shows correct PIDs/names
- musl AND glibc linked `hello-world` program runs
- glibc-linked `pthreads` test (14/14) passes
- `strace -e trace=file /bin/ls /` works (reading /proc during syscall tracing)

**Test binaries:**
- glibc-compiled version of our 14-test suite (mini_threads)
- glibc-compiled `ps`, `top`, `strace` from busybox or Alpine
- Python 3.x (to test glibc + malloc + /proc/maps interaction)

## Known Challenges

1. **Dynamic /proc inode allocation:** `/proc/[pid]/` shouldn't exist until the
   process is created, and must disappear when it exits. This requires VFS
   support for on-demand inode creation in directory listing.
2. **/proc/[pid]/fd/ symlinks:** Must resolve to the actual file path, requiring
   reverse-lookup in the inode table (not all filesystems track this).
3. **glibc futex semantics:** `FUTEX_CMP_REQUEUE` is complex (atomically check
   value, wake some waiters, requeue others). We need to be very careful with
   the implementation to avoid lost wakeups.
4. **Backward compatibility:** musl tests must still pass after glibc changes.
   Both libcs use futex, but with different syscall argument patterns.

## Success Metrics

- [ ] 14/14 glibc pthreads tests pass
- [ ] All 14/14 musl pthreads tests still pass
- [ ] `/proc/[pid]/maps` shows correct memory layout
- [ ] `ps aux` works and shows correct processes
- [ ] `python3 -c "import ctypes; print(ctypes.CDLL)"` works (requires /proc/maps)
- [ ] strace can trace a simple program
- [ ] No glibc-specific changes to core kernel — all changes in VFS or syscalls
