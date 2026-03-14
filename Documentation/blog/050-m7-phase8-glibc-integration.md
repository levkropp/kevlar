# M7 Phase 8: glibc Integration

Phase 8 brings glibc compatibility to Kevlar.  Static glibc binaries
now run on Kevlar, including glibc's NPTL pthreads on 4 CPUs.

## glibc hello world

A statically-linked glibc hello world (Ubuntu 20.04, glibc 2.31,
gcc 9.3) boots and runs to completion on Kevlar.  This exercises
glibc's full init sequence: `__libc_start_main`, TLS setup,
`set_tid_address`, `set_robust_list`, signal mask initialization,
buffered stdio, and `exit_group`.

## glibc pthreads: 14/14

The existing `mini_threads.c` test suite, compiled with `gcc -static
-pthread` instead of `musl-gcc`, passes all 14 tests on `-smp 4`:

thread_create_join, gettid_unique, getpid_same, shared_memory,
atomic_counter, mutex, tls, condvar, signal_group, tgkill,
mmap_shared, fork_from_thread, pipe_pingpong, thread_storm.

The condvar test passing confirms FUTEX_CMP_REQUEUE works correctly
under glibc's NPTL implementation.  The tgkill test confirms targeted
signal delivery to specific threads works with glibc's thread model.

## Signal bounds fix

glibc's signal handling uses real-time signal numbers (32+) in its
internal bookkeeping.  The kernel's `SignalDelivery` array was sized
for signals 1-31 (SIGMAX=32, 32-element array with indices 0-31).
When glibc set signal 32's action via `rt_sigaction`, `set_action`
indexed past the array, causing a panic.

Fixed by:
- `set_action`: reject signal >= SIGMAX with EINVAL (was > SIGMAX)
- `get_action`: return Ignore for out-of-range signals
- `pop_pending` / `pop_pending_unblocked`: skip signals beyond array

## Build system

New Dockerfile stages build glibc test binaries:
- `hello_glibc`: `gcc -static -O2`
- `mini_threads_glibc`: `gcc -static -O2 -pthread`

New Makefile targets:
- `make test-glibc-hello` — single-process glibc test
- `make test-glibc-threads` — 14-test pthreads suite on 4 CPUs
- `make test-m7` — full M7 integration suite

## Results

- glibc hello: PASS
- glibc pthreads: 14/14 on -smp 4
- musl pthreads: 14/14 (no regression)
- Contracts: 26/26 PASS, 0 DIVERGE
