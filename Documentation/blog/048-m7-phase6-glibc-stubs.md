# M7 Phase 6: glibc Syscall Stubs

Phase 6 adds the syscall stubs that glibc calls during early
initialization.  Without these, glibc-linked binaries hit
"unimplemented syscall" warnings and may crash before reaching main().

## The problem

glibc 2.34+ probes several kernel features during libc init:

- **rseq** (restartable sequences) — glibc tries to register an rseq
  area; if the kernel returns ENOSYS, glibc falls back gracefully.
- **clone3** — glibc's pthread_create tries clone3 first, falls back
  to clone on ENOSYS.
- **sched_setaffinity** — called after clone() to set thread affinity.
- **sched_getscheduler / sched_setscheduler** — queried during init
  to determine scheduling capabilities.

None of these need real implementations yet — correct error codes and
no-op stubs are sufficient for glibc to proceed past init.

## Implementation

Five new syscall files, each trivial:

| Syscall | x86_64 | arm64 | Behavior |
|---------|--------|-------|----------|
| rseq | 334 | 293 | Returns ENOSYS |
| clone3 | 435 | 435 | Returns ENOSYS |
| sched_setaffinity | 203 | 122 | No-op, returns 0 |
| sched_getscheduler | 145 | 121 | Returns 0 (SCHED_OTHER) |
| sched_setscheduler | 144 | 119 | No-op, returns 0 |

`set_robust_list` was already implemented during M2.

## Contract test

The `glibc_stubs.c` test verifies Linux-identical behavior for all
five stubs:

- rseq with null args returns EINVAL
- sched_setaffinity succeeds (returns 0)
- sched_getscheduler returns SCHED_OTHER (0)
- sched_setscheduler succeeds (returns 0)
- clone3 with null args returns EFAULT

The stubs match Linux's argument validation: rseq returns EINVAL for
null/undersized args (before it would return ENOSYS for a valid
registration), and clone3 returns EINVAL for size < 64 bytes (before
it would return ENOSYS for a properly-sized struct).  This means the
invalid-args contract tests produce identical results on both kernels.
glibc's fallback path still works because it passes valid args and
gets ENOSYS.

## Known divergences mechanism

Phase 6 also introduces `known-divergences.json` and XFAIL support in
the contract test runner.  Tests listed in the file still run and show
their output, but are reported as XFAIL instead of DIVERGE/FAIL and
don't cause a non-zero exit code.  This makes gaps visible without
blocking CI.  Currently no tests need it.

## Results

25/25 contract tests pass, zero divergences.

## What's next

Phase 7 adds the missing futex operations (CMP_REQUEUE, WAKE_OP,
WAIT_BITSET) that glibc's NPTL threading library requires for
condition variables and timed waits.
