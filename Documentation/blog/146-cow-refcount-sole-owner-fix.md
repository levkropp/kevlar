# Blog 146: Two COW bugs — the missing writable bit and the phantom refcount

**Date:** 2026-04-06
**Milestone:** M11 Alpine Graphical — Stability

## Summary

The ip=0 crash that killed every process after `fork()` had two
root causes in the copy-on-write page fault handler:

1. **refcount==1 no-op** — when a COW page's refcount dropped to 1
   (sole owner), the handler did nothing instead of restoring the
   WRITABLE bit. The page stayed read-only, causing infinite fault
   loops that eventually corrupted the parent's stack. **Fixed.**

2. **Phantom refcount** — a physical page has refcount=1 but two
   processes map it. A global canary caught PID 37 writing to PID
   1's private stack page. This means `page_ref_inc` during fork
   misses an increment or `page_ref_dec` has an extra decrement.
   **Under investigation.**

## Bug 1: The sole-owner no-op

After `fork()`, both parent and child share pages with refcount=2
and PTEs marked read-only. When either process writes:

1. Page fault (write to read-only page)
2. COW handler checks refcount
3. If refcount > 1: allocate new page, copy, decrement old refcount
4. If refcount == 1: **sole owner — just make writable**

Step 4 is the critical fix. The original code:

```rust
if refcount > 1 || is_ghost {
    // ... copy page, map new, return ...
}
// refcount == 1 and not ghost: sole owner, just make writable.
// ← NOTHING HERE! Falls through!
```

The comment says "just make writable" but the code does nothing.
The page stays read-only. The CPU retries the write. Another fault.
The handler allocates a fresh zeroed page via `try_map`, which
returns false (page already mapped). The fresh page is freed. The
COW check runs again. refcount still 1. Falls through again.
Infinite loop.

Eventually, the page allocator's free/alloc cycling causes the
same physical page to be allocated to a different process, which
writes to it — corrupting the original owner's data.

**Fix:**

```rust
// refcount == 1 and not ghost: sole owner, just make writable.
vm.page_table_mut().update_page_flags(aligned_vaddr, prot_flags);
vm.page_table().flush_tlb_local(aligned_vaddr);
return;
```

## Bug 2: The phantom refcount

Even with Bug 1 fixed, fork() still corrupts pages ~80% of the
time. A global canary proved that PID 1's private physical page
(refcount=1) is written to by a child process (PID 37).

### The canary

```rust
static PID1_STACK_PADDR: AtomicU64 = AtomicU64::new(0);

// On EVERY page fault from ANY process:
let watched = PID1_STACK_PADDR.load(Relaxed);
if watched != 0 {
    let val = unsafe { *((watched + KERNEL_BASE + 0xbd8) as *const u64) };
    if val == 0x6c76656b00000000 {  // "kevl"
        panic!("CANARY: pid={} corrupted PID1 stack paddr={:#x} refcount={}",
               pid, watched, page_ref_count(watched));
    }
}
```

Result:
```
GLOBAL CANARY: pid=37 triggered! PID1 stack paddr=0x3ab2f000
has 'kevlar'! refcount=1
```

### What this means

- PID 1 owns page `0x3ab2f000` with refcount=1 (sole owner)
- PID 37 (a child process) ALSO maps this physical page
- PID 37's `uname()` syscall writes "kevlar" (the hostname) to
  what PID 37 thinks is its own stack — but it's PID 1's page
- The refcount should be 2 (both processes map it) but it's 1

The refcount underflow means `page_ref_inc` during
`duplicate_table_cow` missed incrementing this page, OR
`page_ref_dec` was called one extra time during a previous COW.

### Investigation so far

- The "kevlar" does NOT appear during any COW copy (all pages
  scan clean at COW time)
- The corruption does NOT appear at any syscall entry for PID 1
- The corruption does NOT appear during timer interrupt checks
  for PID 1
- The corruption IS caught by the global canary during PID 37's
  page fault handler (the first kernel code that runs after the
  corruption)

This means the corruption happens during USER-MODE execution of
PID 37 — the `copy_to_user` in PID 37's `uname()` syscall writes
through PID 37's page table, which has a PTE pointing to PID 1's
physical page.

## Timeline of the corruption

```
1. PID 1 forks → child shares stack page P (refcount=2)
2. PID 1 COW → PID 1 gets P' (refcount=1), P refcount→1
3. Child COW → child gets P'' (refcount=1), P refcount→0, P freed
4. PID 1 forks again → shares P' (refcount should be 2)
   BUT: refcount(P') only went to 1 (missed increment!)
5. PID 1 COW → PID 1 gets P''' (refcount=1), P' refcount→0, P' freed
6. P' gets reallocated to a grandchild as stack page
7. Grandchild's uname() writes "kevlar" to P' → PID 1 maps P' → corruption
```

The bug is in step 4: `duplicate_table_cow` should increment P'
from 1 to 2, but somehow misses it. This causes the cascade that
leads to P' being freed while PID 1 still maps it.

## Workarounds

- `vfork()` avoids the race entirely (parent blocks until child
  execs, no concurrent page access)
- Adding page fault overhead (diagnostic canary code) changes
  timing enough to mask the bug
- Disabling PCID reduces (but doesn't eliminate) the crash rate

## Remaining work

The phantom refcount bug is the #1 priority. The
`duplicate_table_cow` function's batch null-check optimization
(8-entry scan per cache line) might be skipping pages under certain
alignment conditions, failing to increment their refcounts.
