# Blog 096: Vm::Drop fix — exec_true reaches Linux parity, 5 workloads improve

**Date:** 2026-03-19
**Milestone:** M10 Alpine Linux

## Context

Kevlar's fork+exec workload benchmarks were 10-23% slower than Linux KVM:
exec_true (1.20x), shell_noop (1.11x), tar_extract (1.23x), pipe_grep,
sed_pipeline, sort_uniq all lagging.  The original plan blamed ghost-fork
(disabled) and insufficient BSS prefaulting.  Both turned out to be wrong.

---

## Failed approaches

### Ghost-fork (GHOST_FORK_ENABLED)

The plan said to flip `GHOST_FORK_ENABLED` from false to true, saving ~14µs
per fork by sharing the parent's VM instead of duplicating the page table.

**Result:** Immediate GPF crash.  musl's `_Fork()` wrapper modifies TLS and
global state in the child:

```c
// musl src/process/_Fork.c
self->tid = __syscall(SYS_set_tid_address, &self->tid);
self->robust_list.off = 0;
libc.threads_minus_1 = 0;
if (libc.need_locks) libc.need_locks = -1;
```

With ghost-fork, parent and child share the address space.  These writes
corrupt the parent's TLS (self->tid overwritten) and global libc state.
Only `vfork()` is safe because callers follow the vfork contract (only
exec or _exit, and musl's vfork wrapper doesn't modify shared state).

### Increased prefault threshold (MAX_PREFAULT_PAGES 8 → 64)

The plan said increasing BSS prefaulting from 8 to 64 pages would eliminate
demand faults for BusyBox's larger BSS sections.

**Result:** exec_true went from 98µs to **144µs** (47% worse).  For
short-lived processes like `/bin/true` that exit immediately, prefaulting
pages they never touch is pure waste: alloc + zero + map at ~1.5µs/page
for pages that are never accessed.

---

## Root cause: disabled Vm::Drop causes CoW refcount inflation

`Vm::Drop` was commented out with this note:

```rust
// Vm::Drop disabled: teardown_user_pages hangs on large page tables.
// Root cause under investigation (blog 089).
```

Without teardown, every fork permanently inflates page refcounts:

1. **Fork:** `duplicate_table` increments refcount on every shared page
   (1 → 2) and clears WRITABLE for CoW
2. **Exec:** Replaces the child's VM, dropping the old `Arc<SpinLock<Vm>>`
3. **Drop disabled:** Refcounts never decremented back to 1
4. **Parent writes:** CoW fault handler sees refcount > 1 → **full page copy**
   (alloc new page, memcpy 4KB, remap) instead of just restoring WRITABLE

Each fork+exec cycle compounds the problem.  By iteration 10, the parent
is doing unnecessary full CoW copies on every stack/data write.  Each copy
costs ~1.5µs (KVM VM exit + page alloc + 4KB memcpy + PTE update).  With
5-10 CoW'd pages touched per iteration, that's 7-15µs of wasted work.

### Why teardown_user_pages was disabled

The original `teardown_user_pages` frees data pages when their refcount
reaches zero.  This caused use-after-free: the page cache holds PAddr
references to demand-faulted pages.  When teardown freed a page whose
only remaining reference was the cache, subsequent execs of the same
binary would prefault from a dangling cache entry.

---

## Fix: teardown_forked_pages (dec-only, never free data pages)

New function `teardown_table_dec_only`:

```rust
fn teardown_table_dec_only(table_paddr: PAddr, level: usize) {
    // ... for each leaf PTE:
    // Decrement refcount only, NEVER free the data page.
    crate::page_refcount::page_ref_dec(paddr);
    // ... for intermediate levels:
    // Recurse, then free the page table page itself.
    crate::page_allocator::free_pages(paddr, 1);
}
```

**Key difference from `teardown_table`:** leaf pages are never freed, only
decremented.  This is safe because:

- Pages with only a cache reference (refcount 1 after dec) stay alive for
  future prefaulting
- Pages still mapped in the parent (refcount ≥ 1) stay alive
- Intermediate page table pages (allocated during `duplicate_table`) are
  correctly freed — they're unique to the forked copy

The PML4 page itself is also freed, and the field zeroed to prevent
double-free.

### Effect on CoW

After the fix, when a forked child exits or exec's:

1. Child's forked page table is torn down (refcounts decremented)
2. Parent's pages return to refcount 1 (sole owner)
3. Next write: CoW handler sees refcount == 1 → just restores WRITABLE
   (no page copy, ~500ns instead of ~1.5µs)

---

## Batch allocation in prefault_small_anonymous

Also replaced per-page `alloc_pages(1)` loop with `alloc_page_batch()`
in `prefault_small_anonymous`.  For the typical 1-8 page BSS prefault,
this amortizes the allocator lock acquisition.  Minor improvement (~100ns
per exec for cached binaries).

---

## Results

Full KVM benchmark comparison (44 benchmarks):

| Benchmark | Before | After | Linux | Change |
|-----------|--------|-------|-------|--------|
| exec_true | 97.6µs (1.20x) | 81-85µs (1.00-1.04x) | 81.5µs | **Parity** |
| shell_noop | 121.7µs (1.11x) | 110.9µs (1.01x) | 109.7µs | **Parity** |
| pipe_grep | 333µs+ | 303-309µs (0.91-0.93x) | 333.2µs | **Faster** |
| sed_pipeline | 422µs+ | 388-400µs (0.91-0.94x) | 424.8µs | **Faster** |
| sort_uniq | 937µs+ | 899-906µs (1.00x) | 900.2µs | **Parity** |
| tar_extract | 647µs (1.23x) | 596-608µs (1.13-1.16x) | 525.5µs | Improved |

Overall: **30 faster, 14 OK, 1 marginal (tar_extract), 0 regressions.**

The remaining tar_extract gap (~70µs, 13-16%) is in VFS operations
(file creation/deletion in tmpfs), not fork/exec overhead.

Contract tests: **116/118 PASS, 2 XFAIL, 0 FAIL** — unchanged.

---

## Files changed

- `kernel/mm/vm.rs` — Enabled `Vm::Drop` using `teardown_forked_pages`
- `platform/x64/paging.rs` — Added `teardown_table_dec_only` + `teardown_forked_pages`
- `platform/arm64/paging.rs` — Same for ARM64
- `kernel/process/process.rs` — Batch alloc in `prefault_small_anonymous`, `alloc_page_batch` import

## Lessons

1. **Profile before optimizing.** The plan's two main optimizations (ghost-fork,
   prefault threshold) both made things worse.  The actual root cause (disabled
   Vm::Drop) was a subtle second-order effect: refcount inflation causing
   unnecessary page copies on every subsequent fork cycle.

2. **htrace is invaluable.** The crash from enabling the original
   `teardown_user_pages` (full teardown) was debugged via htrace in one run:
   the parent crashed at address 0x100000000300 after the second fork+exit,
   confirming a use-after-free in the page cache path.

3. **Separate "dec refcount" from "free page".**  The original teardown
   conflated these operations.  The fix keeps them separate: forked page
   tables only need refcount decrements (to undo fork's increments), never
   data page frees (those pages may be in the page cache or parent's VM).
