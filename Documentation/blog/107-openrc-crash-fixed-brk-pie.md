# Blog 107: OpenRC crash fixed — brk() was broken for all PIE binaries

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

## Investigation

Tracing showed:
1. **No mmap or brk calls** from the crashing PIDs — they crashed on
   first malloc in a freshly forked process
2. The faulting address `0xa001X8008` was always ~1.5MB above the
   loaded image, in musl's malloc free-list traversal code
3. **brk tracing** revealed the root cause: `ok=false` for every
   single brk expansion across ALL PIE processes (PIDs 1-28)

## Root Cause

**brk() always failed for PIE binaries.** The heap expansion guard
compared `new_heap_end >= stack_bottom`:

```
heap_bottom = 0xa0016d000  (in valloc region, above 0xa00000000)
stack_bottom = 0x9fffff0000 (below valloc base)
→ 0xa0016f000 >= 0x9fffff0000 → ALWAYS TRUE → brk rejected
```

For PIE binaries, the kernel places the heap in the valloc region
(after the loaded ELF image at `0xa000XXXXX`). The stack is below
the valloc base. The guard intended to prevent heap-stack collision
was rejecting ALL heap growth because the heap was numerically above
the stack.

musl's malloc calls brk() first. When it fails, malloc falls back
to mmap for large allocations but keeps broken metadata pointers
into the failed-brk region. The first dereference of these pointers
crashes with "no VMA."

## The Fix

When `heap_bottom >= stack_bottom` (PIE layout), use `USER_VALLOC_END`
as the limit instead of `stack_bottom`:

```rust
let limit = if self.heap_bottom >= stack_bottom {
    USER_VALLOC_END  // PIE: heap in valloc, can't collide with stack
} else {
    stack_bottom     // non-PIE: classic heap-grows-up-stack-grows-down
};
```

## Other Fixes This Session

### `__WCLONE` in wait4
musl's `posix_spawn` calls `wait4(pid, &status, __WCLONE, 0)` after
`clone(CLONE_VM|CLONE_VFORK|SIGCHLD)`. On Linux, `__WCLONE` only
matches children with non-SIGCHLD exit signals — since ours use
SIGCHLD, it should return ECHILD immediately. Our kernel was
stripping `__WCLONE` via bitflags truncation, turning it into a
blocking wait that prematurely reaped the child.

### clone() CLONE_VM dispatch
`clone(CLONE_VM)` without `CLONE_THREAD` (used by posix_spawn)
was dispatching to the `new_thread` path which shares fd tables.
Fixed to correctly require `CLONE_THREAD` for the thread path.

### brk VMA extension
Consecutive brk expansions could fail when the adjacent VMA check
returned "not free" for the previous allocation's boundary. Now
extends the existing anonymous VMA instead of failing.

---

## Results

| Metric | Before | After |
|--------|--------|-------|
| OpenRC crashes per boot | 4 | **0** |
| brk success for PIE | 0% | **100%** |
| Alpine boot | crash+recover | **clean** |
| Processes killed by SIGSEGV | 4+ | **0** |
