# Milestone 9.8: Comprehensive Systemd Drop-In Validation

**Goal:** Raise Kevlar's systemd support from a 4-check smoke test (`test-m9`)
to a comprehensive 25-test synthetic init-sequence suite plus real systemd boot,
providing confident evidence that Kevlar is a genuine Linux kernel replacement
for systemd workloads.

**Current state:** M9 achieved real systemd booting — `test-m9` runs 4 grep-string
checks with a 20s timeout. Several kernel bugs and missing syscalls cause failures
in the 25-test `mini_systemd_v3` suite and limit systemd stability.

**Impact:** After M9.8, `make test-systemd` chains a 25-test synthetic
init-sequence suite (single + SMP) with a real systemd boot check. This gives
high confidence that new kernel changes won't regress systemd compatibility.

## Phases

| Phase | Name | Description | Prerequisite |
|-------|------|-------------|--------------|
| [1](phase1-kernel-fixes.md) | Kernel Bug Fixes | Stable boot_id, real rt_sigtimedwait, FIOCLEX/FIONCLEX, osrelease check | M9 |
| [2](phase2-syscall-dispatch.md) | Missing Syscall Dispatch | clock_nanosleep, clock_getres, timerfd_gettime, ENOSYS stubs | Phase 1 |
| [3](phase3-procfs-additions.md) | procfs Additions | kptr_restrict, dmesg_restrict, /proc/sys/vm/ | Phase 1 |
| [4](phase4-test-infrastructure.md) | Test Infrastructure | test-systemd-v3-smp, upgrade test-m9, test-systemd meta-target | Phases 1-3 |

## Blockers Addressed

1. `/proc/sys/kernel/random/boot_id` regenerates on every read — systemd expects a stable UUID
2. `rt_sigtimedwait` yields once then returns EAGAIN — systemd uses it to wait for SIGCHLD
3. `FIOCLEX`/`FIONCLEX` ioctls (0x5451/0x5450) not handled — fall through to per-file ioctl
4. `mini_systemd_v3.c` test 23 checks `osrelease` for "4.0.0" but kernel returns "6.19.8"
5. `clock_nanosleep`, `clock_getres`, `timerfd_gettime` missing from dispatch table
6. `test-m9` timeout is 20s — real systemd needs 60-90s under KVM
7. No `test-systemd-v3-smp` or `test-systemd` meta-target exists

## Critical Files

| File | Phase | Change |
|------|-------|--------|
| `kernel/fs/procfs/mod.rs` | 1, 3 | boot_id stable; kptr_restrict, dmesg_restrict, vm/ subdir |
| `kernel/syscalls/mod.rs` | 1, 2 | replace rt_sigtimedwait stub; add 5 dispatch entries + constants |
| `kernel/syscalls/rt_sigtimedwait.rs` | 1 | new file — real implementation |
| `kernel/syscalls/ioctl.rs` | 1 | FIOCLEX/FIONCLEX |
| `testing/mini_systemd_v3.c` | 1 | osrelease check fix |
| `kernel/syscalls/nanosleep.rs` | 2 | sys_clock_nanosleep |
| `kernel/syscalls/clock_gettime.rs` | 2 | sys_clock_getres |
| `kernel/syscalls/timerfd.rs` | 2 | sys_timerfd_gettime |
| `kernel/fs/timerfd.rs` | 2 | TimerFd::gettime() |
| `Makefile` | 4 | test-systemd-v3-smp, upgrade test-m9, test-systemd |

## Implementation Order

1. **1.4** — mini_systemd_v3.c osrelease fix (1-line, establishes baseline)
2. **1.1** — boot_id (single file, unblocks test 23)
3. **1.3** — FIOCLEX/FIONCLEX (5-line addition)
4. **Phase 2** — syscall stubs (mod.rs + small helpers)
5. **Phase 3** — procfs additions (single file)
6. **1.2** — rt_sigtimedwait (most complex kernel work)
7. **Phase 4** — Makefile test targets (pure text)

`make check` after each kernel phase.

## Verification

```bash
# Incremental:
make check                              # after each phase
make RELEASE=1 test-systemd-v3         # expect TEST_END 25/25

# Final:
make RELEASE=1 test-systemd-v3         # 25/25, 1 CPU
make RELEASE=1 test-systemd-v3-smp     # 25/25, 4 CPUs
make RELEASE=1 test-m9                 # 4/4 checks
make RELEASE=1 test-systemd            # all three pass

# Regressions:
make RELEASE=1 test-busybox            # must remain 101/101
make RELEASE=1 test-busybox-smp        # must remain 101/101
make RELEASE=1 test-contracts          # ≥104 PASS, 8 XFAIL, 0 FAIL
```

## Expected Final Output

```
$ make RELEASE=1 test-systemd
[TEST] M9.8: comprehensive systemd drop-in validation
Step 1/3: synthetic init-sequence (1 CPU)
TEST_END 25/25
ALL SYSTEMD-V3 TESTS PASSED
Step 2/3: synthetic init-sequence SMP (4 CPUs)
TEST_END 25/25
ALL SYSTEMD-V3 SMP TESTS PASSED
Step 3/3: real systemd PID 1 boot
PASS: Welcome banner
PASS: Startup finished
PASS: Reached target Kevlar Default Target
PASS: Started Kevlar Console Shell
4/4 required checks passed
=== M9.8 test-systemd: ALL PASSED ===
```
