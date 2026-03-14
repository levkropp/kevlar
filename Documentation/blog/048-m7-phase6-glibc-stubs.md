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

The `glibc_stubs.c` test calls the three stubs that produce identical
results on both kernels:

- sched_setaffinity succeeds (returns 0)
- sched_getscheduler returns SCHED_OTHER (0)
- sched_setscheduler succeeds (returns 0)

rseq and clone3 are not contract-tested because they return ENOSYS on
Kevlar (not yet implemented) vs EINVAL/EFAULT on Linux (implemented
but rejecting invalid args).  Full implementations will come later.

## Results

25/25 contract tests pass with zero divergences from Linux.

## What's next

Phase 7 adds the missing futex operations (CMP_REQUEUE, WAKE_OP,
WAIT_BITSET) that glibc's NPTL threading library requires for
condition variables and timed waits.
