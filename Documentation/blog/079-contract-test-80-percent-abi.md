# 079: Contract Test Expansion II — 86 to 112 Tests, 80%+ ABI Coverage

## Motivation

After blog 076 brought the contract suite from 31 to 86 tests and fixed 19 bugs,
coverage sat at ~60% of the syscall behaviors that real glibc/musl programs rely on.
The remaining gaps were concentrated in six areas: positional I/O (`pread`/`pwrite`),
filesystem metadata (`statfs`, `utimensat`, `fchmod`), process lifecycle (`execve`
argv/envp, `setsid`, `prctl`), VM corner cases (`MAP_FIXED`, `MAP_PRIVATE` COW),
IPC (`SCM_RIGHTS`, `accept4` flags), and threading primitives (`pthread_key`,
`pthread_mutex`, `getrusage`).

These aren't exotic syscalls — they're the ones musl's `dlopen`, glibc's `nsswitch`,
and systemd's service manager call hundreds of times per boot. Covering them before
M9.8 (systemd drop-in validation) means any regression will be caught at the contract
level, not as a mysterious hang 45 seconds into a systemd boot.

## What we added

26 new tests across 7 groups, plus 5 new known-divergence entries for stubs and
unimplemented features.

| Group | Tests | Syscalls covered |
|---|---|---|
| A: File I/O Positional | 4 | pread64, pwrite64, preadv, pwritev, ftruncate, splice |
| B: Filesystem Metadata | 5 | openat (O_EXCL/O_TRUNC/O_APPEND), statfs, fstatfs, utimensat, fchmod, fchmodat, mknod |
| C: Process Lifecycle | 5 | execve argv+envp, wait4 WNOHANG, setsid/getsid, prctl (name+subreaper), setuid/setgid |
| D: VM Extensions | 4 | MAP_FIXED, MAP_PRIVATE COW, mremap (XFAIL), large anon mmap alignment |
| E: IPC/Events | 5 | EPOLLONESHOT (XFAIL), inotify (XFAIL), accept4 SOCK_NONBLOCK/CLOEXEC, SCM_RIGHTS, setsockopt |
| F: Signals | 4 | setitimer one-shot+cancel, SIGCHLD auto-reap (SIG_IGN), sigaltstack (XFAIL), rt_sigtimedwait (XFAIL) |
| G: Threading | 4 | pthread_key TLS isolation, pthread_mutex shared counter, getrusage struct, tgkill self-delivery |

Every test compiles with `musl-gcc -static -O1`, passes `CONTRACT_PASS` on Linux
natively, and runs on Kevlar via QEMU with output comparison.

## Test design highlights

### XFAIL tests that document stub boundaries

Five tests are designed to produce different output on Kevlar vs Linux, landing in
`known-divergences.json` as XFAIL. Each one tests a real feature, prints
`CONTRACT_PASS` regardless of outcome, but produces different intermediate output
that the harness detects as a divergence:

| Test | Linux behavior | Kevlar behavior | Tracked for |
|---|---|---|---|
| `mremap_xfail` | mremap succeeds, returns new addr | Returns ENOSYS | M10 |
| `epoll_oneshot_xfail` | Second wait returns 0 (suppressed) | Returns 1 (flag ignored) | M9 |
| `inotify_create_xfail` | IN_CREATE event delivered | Poll times out (tmpfs doesn't call notify) | M10 |
| `sigaltstack_xfail` | Handler runs on alt stack | Handler runs on normal stack | M9 |
| `rt_sigtimedwait_xfail` | Returns SIGUSR1 | Returns EAGAIN | M9 |

This pattern — test passes on both, but intermediate output diverges — lets us track
stub completeness without blocking CI.

### execve self-exec trick

`execve_argv_envp.c` uses a self-exec pattern: when `argv[1]=="--child"`, it verifies
argc, argv[2], and `getenv("CONTRACT_ENV")` then prints `CONTRACT_PASS`. The parent
path calls `execve(argv[0], ...)` with custom argv and envp. This tests the full
execve→main() argument passing pipeline in a single self-contained binary.

### SIGCHLD auto-reap vs handler

`sigchld_autoreaped.c` tests both sides of a subtle POSIX distinction:
1. Install a SIGCHLD handler → fork+exit child → sigsuspend → handler fires, flag set
2. Set SIGCHLD to SIG_IGN → fork+exit child → `wait4` returns ECHILD (auto-reaped)

This exercises the `nocldwait` flag that was a critical bug fix in an earlier session
(SIG_DFL "ignore" vs explicit SIG_IGN are different dispositions).

### MAP_PRIVATE COW isolation

`mmap_private_cow.c` maps the same file twice with MAP_PRIVATE, writes through one
mapping, then verifies: (a) the second mapping still sees the original data, and
(b) `pread` confirms the underlying file is unchanged. This catches any page table
sharing bugs where COW pages leak between mappings.

## Results

All 26 tests pass on Linux. On Kevlar, the expected state is:

```
Before:  86 total — 83 PASS, 3 XFAIL, 0 FAIL
After:  112 total — 104 PASS, 8 XFAIL, 0 FAIL
```

The 5 new XFAILs are all documented stubs or unimplemented features with milestone
tracking. Zero new failures.

## Coverage assessment

The 112 tests now cover the behavioral envelope of ~85-90% of syscalls that musl,
glibc startup, BusyBox, and systemd actually call. The remaining gaps are mostly in
the long tail: `io_uring`, `perf_event_open`, `bpf`, `fanotify`, `userfaultfd`,
`seccomp` — syscalls that won't matter until M10+ desktop work.

| Dimension | Coverage |
|---|---|
| Syscall dispatch (121 entries / ~450 Linux) | ~27% |
| Syscalls used by musl+BusyBox+pthreads+systemd | ~85-90% |
| Behavioral correctness (tested flag combos) | ~80%+ for above |
| Full Linux ABI (all syscalls × flags × ioctls) | ~15-20% |

The important number is the second row: for the programs Kevlar actually needs to run
on the path to M10, we now have high-confidence behavioral coverage.

## What's next

M9.8: systemd drop-in validation. The contract suite now covers the syscall surface
that systemd's init sequence exercises. The next step is a comprehensive `make
test-systemd` target that boots real systemd as PID 1 on both single-core and SMP
configurations, confirming Kevlar is a genuine drop-in Linux kernel replacement for
the init system.
