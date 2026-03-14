# Milestone 7: glibc Compatibility & /proc Filesystem

**Goal:** Run glibc-linked binaries and provide system introspection via
/proc.  This enables distro programs (ps, top, strace) and glibc-linked
binaries to work on Kevlar.

**Current state:** Kevlar runs musl-linked static binaries flawlessly
(14/14 pthreads tests, 18/19 M6.5 contracts).  glibc-linked binaries
fail early in initialization (missing futex ops, rseq).  /proc exists
minimally (/proc/self/stat, /proc/self/exe) but most files are missing.

**Impact:** glibc is the libc for ~95% of distro binaries.  /proc is
read at startup by nearly every process.  Together they gate M8
(cgroups), M9 (systemd), and M10 (desktop).

## Phase breakdown (8 phases)

| Phase | Scope | Duration |
|-------|-------|----------|
| 1 | /proc VFS skeleton + mount + /proc/self symlink | 1 day |
| 2 | Global /proc files: cpuinfo, version, meminfo, mounts | 1 day |
| 3 | Per-process: /proc/[pid]/stat, status, cmdline | 1.5 days |
| 4 | /proc/[pid]/maps | 1 day |
| 5 | /proc/[pid]/fd/ directory + symlinks | 1 day |
| 6 | glibc syscall stubs: rseq, sched_setaffinity, sched_*scheduler | 0.5 day |
| 7 | Futex operations: CMP_REQUEUE, WAKE_OP, WAIT_BITSET | 2 days |
| 8 | Integration testing: glibc hello, pthreads, ps aux | 1.5 days |
| **Total** | | **~10 days** |

## Why /proc + glibc together

glibc and /proc are tightly coupled:
- glibc's malloc reads `/proc/self/maps` to understand memory layout
- Python's ctypes reads `/proc/self/maps` to find loaded libraries
- systemd reads `/proc/*/stat` to track processes
- `ps`, `top`, `htop` are completely /proc-dependent
- glibc NPTL uses `FUTEX_CMP_REQUEUE` for condition variables

## Syscalls involved

- **futex (240):** FUTEX_CMP_REQUEUE (op 4), FUTEX_WAKE_OP (op 5), FUTEX_WAIT_BITSET (op 9)
- **rseq (334):** Return ENOSYS (glibc handles gracefully)
- **sched_setaffinity (203):** No-op stub
- **sched_getscheduler (145):** Return SCHED_OTHER
- **sched_setscheduler (144):** No-op stub
- **VFS syscalls:** openat, read, readdir, readlink for /proc files

## Success criteria

- [ ] glibc hello world runs and exits cleanly
- [ ] 14/14 glibc pthreads tests pass on -smp 4
- [ ] 14/14 musl pthreads tests still pass (no regressions)
- [ ] 18/19 M6.5 contract tests still pass
- [ ] `ps aux` lists processes from /proc
- [ ] `cat /proc/self/maps` shows correct memory layout
- [ ] `/proc/cpuinfo` reports correct CPU count
- [ ] No M6.6 benchmark regressions >10%
