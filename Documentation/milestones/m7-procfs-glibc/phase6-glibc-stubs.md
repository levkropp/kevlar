# Phase 6: glibc Syscall Stubs

**Duration:** ~0.5 day
**Prerequisite:** None (independent of /proc phases)
**Goal:** Stub syscalls that glibc calls during initialization.

## Scope

glibc-linked binaries call several syscalls during libc init that Kevlar
doesn't handle yet.  Most can be trivially stubbed — the real complexity
is in futex ops (Phase 7).

## Syscalls to add

### 1. rseq (334 on x86_64, 293 on arm64)

Restartable sequences — glibc 2.35+ calls this during init.  When it
returns ENOSYS, glibc falls back to non-rseq paths.

```rust
pub fn sys_rseq(&mut self, _rseq: UserVAddr, _len: u32,
                _flags: i32, _sig: u32) -> Result<isize> {
    Err(Errno::ENOSYS.into())
}
```

### 2. sched_setaffinity (203 on x86_64, 122 on arm64)

glibc calls this after clone() to set thread affinity.  No-op is fine.

```rust
pub fn sys_sched_setaffinity(&mut self, _pid: i32,
                              _cpusetsize: usize,
                              _mask: UserVAddr) -> Result<isize> {
    Ok(0)
}
```

### 3. sched_getscheduler (145 on x86_64, 121 on arm64)

Returns the scheduling policy.  Always return SCHED_OTHER (0).

```rust
pub fn sys_sched_getscheduler(&mut self, _pid: i32) -> Result<isize> {
    Ok(0) // SCHED_OTHER
}
```

### 4. sched_setscheduler (144 on x86_64, 119 on arm64)

Sets the scheduling policy.  No-op stub.

```rust
pub fn sys_sched_setscheduler(&mut self, _pid: i32,
                               _policy: i32,
                               _param: UserVAddr) -> Result<isize> {
    Ok(0)
}
```

### 5. set_robust_list (273 on x86_64, 99 on arm64)

glibc NPTL sets robust futex list.  Already implemented (verify).

### 6. clone3 (435 on x86_64, 435 on arm64)

glibc 2.34+ tries clone3 first, falls back to clone on ENOSYS.

```rust
pub fn sys_clone3(&mut self, _cl_args: UserVAddr,
                   _size: usize) -> Result<isize> {
    Err(Errno::ENOSYS.into())
}
```

## Implementation

- Add all syscall constants to x64 and arm64 sections in mod.rs
- Add dispatch entries
- Add name table entries
- Create `kernel/syscalls/rseq.rs`, `kernel/syscalls/sched_setaffinity.rs`, etc.

## Testing

Contract test: `testing/contracts/programs/glibc_stubs.c`
```c
// Call each stubbed syscall via syscall() and verify it doesn't crash
// rseq should return -1 with errno=ENOSYS
// sched_setaffinity should return 0
// clone3 should return -1 with errno=ENOSYS
```

## Success criteria

- [ ] No ENOSYS crash during glibc init (all stubs in place)
- [ ] glibc hello-world gets past libc init (may still fail on futex)
- [ ] musl tests still pass (stubs don't break existing behavior)
