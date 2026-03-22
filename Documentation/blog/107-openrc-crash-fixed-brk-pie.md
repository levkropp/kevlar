# Blog 107: OpenRC crash fixed — brk() was broken for PIE binaries

**Date:** 2026-03-22
**Milestone:** M10 Alpine Linux

## The Bug

Every Alpine boot crashed OpenRC 4 times:
```
SIGSEGV: no VMA for address 0xa00188008 (pid=23, ip=0xa0004620d)
PID 23 (/sbin/openrc sysinit) killed by signal 11
```

OpenRC recovered by restarting, but the crash happened on every
`openrc sysinit`, `openrc boot`, and `openrc default` invocation.

## Root Cause

**brk() always failed for PIE binaries.** The heap expansion check
compared `new_heap_end >= stack_bottom`. For PIE binaries, the heap
is in the valloc region (above `0xa00000000`), while the stack is
below it (around `0x9fffff0000`). Since `0xa0016f000 >= 0x9fffff0000`
is always true, every brk expansion was rejected.

musl's malloc calls brk() as its primary allocator. When brk failed,
malloc fell back to mmap for some allocations but eventually accessed
the failed-brk region (which had no VMA), causing SIGSEGV in malloc's
free-list traversal.

## The Fix

When `heap_bottom >= stack_bottom` (PIE layout where heap is above the
stack), use `USER_VALLOC_END` as the limit instead of `stack_bottom`:

```rust
let limit = if self.heap_bottom >= stack_bottom {
    USER_VALLOC_END  // PIE: heap is in valloc region
} else {
    stack_bottom     // non-PIE: heap grows toward stack
};
if aligned_new >= limit {
    return Err(Errno::ENOMEM.into());
}
```

## Other Fixes This Session

- **`__WCLONE` in wait4**: musl's posix_spawn calls `wait4(__WCLONE)`
  to detect successful exec. Our kernel stripped the flag via bitflags
  truncation, turning it into a blocking wait. Now returns ECHILD
  immediately (correct for SIGCHLD children).

## Result

Alpine boots with **zero crashes**. OpenRC sysinit/boot/default all
complete successfully. The entire init sequence is now clean.
