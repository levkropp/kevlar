# Phase 3: glibc Compatibility

**Duration:** ~2-3 days
**Prerequisite:** Phase 1 (for /proc support during glibc testing)
**Goal:** Enable glibc-linked binaries to run correctly, especially pthreads.

## Current Status

glibc binaries fail during thread creation with "The futex facility returned an
unexpected error code." The issues are:

1. **Missing futex operations:** glibc NPTL uses `FUTEX_CMP_REQUEUE`,
   `FUTEX_WAKE_OP`, and possibly others. We only support FUTEX_WAIT (0) and
   FUTEX_WAKE (1).
2. **rseq syscall (334):** glibc 2.35+ calls this at thread startup. We return
   ENOSYS, which should work, but error path might be buggy.
3. **sched_setaffinity stub:** glibc calls syscall 203 after cloning. We return
   ENOSYS; it should be no-op success.
4. **Signal mask format:** glibc stores masks in task descriptor, we store
   separately — ensure /proc/[pid]/status agrees.

## Implementation Plan

### 1. Futex Operations

Current implementation in `kernel/syscalls/futex.rs`:
```rust
match cmd {
    FUTEX_WAIT => { /* ... */ }
    FUTEX_WAKE => { /* ... */ }
    _ => Err(Errno::ENOSYS.into())
}
```

Add:
- `FUTEX_CMP_REQUEUE` (op 4): Atomically check value at uaddr, wake some
  waiters, requeue others to uaddr2. Used by condition variables + robust
  mutexes. This is complex — requires careful lock ordering.
- `FUTEX_WAKE_OP` (op 5): Wake waiters and perform atomic operation on another
  futex word (used by advanced mutex wakeup). Less critical but prevents
  crashes.
- `FUTEX_WAIT_BITSET` (op 9 + 128 for private): Extended wait with bitmask
  (used by some pthread condvar implementations).

Reference implementation strategy:
- Consult Linux kernel futex.c for the exact semantics
- CMP_REQUEUE must not lose wakeups (hold WaitQueue lock during requeue)
- Test with glibc's test suite or musl's futex tests

### 2. rseq Syscall (334)

Currently returns ENOSYS via the default dispatch catch-all.

Add explicit handler:
```rust
pub fn sys_rseq(
    &mut self,
    uaddr: UserVAddr,
    len: c_uint,
    flags: c_int,
    sig: c_uint,
) -> Result<isize> {
    // For now: return ENOSYS (rseq not supported)
    // glibc handles this gracefully in 2.35-2.37
    // glibc 2.38+ might require it; we'll address if tests fail
    Err(Errno::ENOSYS.into())
}
```

The key is ensuring errno is set correctly so glibc can branch appropriately.

### 3. sched_setaffinity Stub (203)

Add explicit no-op handler:
```rust
pub fn sys_sched_setaffinity(
    &mut self,
    pid: c_int,
    cpusetsize: c_size,
    mask: UserVAddr,
) -> Result<isize> {
    // Stub: always succeed. Real implementation would set CPU affinity.
    // glibc calls this after clone but doesn't fail if it returns EPERM.
    // We return success (0) to avoid unnecessary errors.
    Ok(0)
}
```

Similarly for `sched_getscheduler` (145), `sched_setscheduler` (144) — return
stub values (SCHED_OTHER, priority 0, etc.).

### 4. Signal Mask Compatibility

glibc stores thread signal masks in the task descriptor (TCB at %fs:offset).
We store in Process struct.

Ensure:
- `sigprocmask` syscall updates both locations atomically
- /proc/[pid]/status reports the same mask as what sigprocmask returns
- Signal delivery checks the thread's local mask, not just process-wide

This might already be correct; test to verify.

### 5. Ensure Errno is Set Correctly

When returning an error (negative return value), ensure:
1. The return value in rax is the negative errno (e.g., -38 for ENOSYS)
2. libc wrapper converts this to a positive errno and stores in %fs:(-8)
3. The user-space check `if (errno == ENOSYS) { ... }` works

This is typically automatic if we use `Err(Errno::XYZ.into())` correctly.

## Testing

- Compile glibc-linked `hello-world` and run: should print and exit cleanly
- Compile glibc-linked version of our 14-test pthreads suite
- Run with `test-threads-smp` (14/14 should pass)
- Verify musl tests still pass (no regressions)

## Known Challenges

1. **FUTEX_CMP_REQUEUE complexity:** This is the critical operation. It must
   atomically:
   - Check value at uaddr (compare)
   - Wake N waiters on uaddr
   - Requeue remaining waiters from uaddr to uaddr2
   - All without losing wakeups or getting out-of-order

   Reference Linux for exact semantics (includes handling of futex value
   changes by other threads).

2. **glibc version differences:** glibc 2.35-2.37 vs 2.38+ handle rseq
   differently. If tests fail, we might need to implement rseq or detect
   version and adapt.

3. **Backward compatibility:** Ensure changes don't break musl (which uses
   simpler futex patterns). Test both continuously.

## Integration Points

- **Syscall dispatch:** Add handlers for futex ops, rseq, sched_* stubs
- **Futex implementation:** Extend WaitQueue model to handle requeue
- **Signal masks:** Verify glibc/musl compatibility in sigprocmask path
