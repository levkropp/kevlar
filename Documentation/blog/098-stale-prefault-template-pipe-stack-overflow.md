# Blog 098: Stale prefault template + pipe stack overflow — 0 REGRESSION, 32 faster

**Date:** 2026-03-20
**Milestone:** M10 Alpine Linux

## Context

After the pipe buffer increase (4KB → 64KB, blog 097 era), two problems
appeared:

1. **sort_uniq and tar_extract hang** when run as benchmarks #43-44 in the
   full 44-benchmark suite (work fine individually)
2. **pipe_grep, sed_pipeline, shell_noop regressed 10-21%** vs Linux KVM

The hang had an obvious diagnosis (stack overflow from the 65KB pipe
buffer).  The regressions required deeper investigation — the root cause
turned out to be a cache coherency bug in the exec prefault template that
had been silently wasting ~15-40µs per exec since the template was
introduced.

---

## Bug 1: Pipe buffer stack overflow

### Symptom

`Box::new(PipeInner { buf: RingBuffer::new(), ... })` constructs a 65KB
`PipeInner` on the kernel stack as a `Box::new` argument, then moves it
to the heap.  With a 16KB kernel stack, this works when the call stack is
shallow (pipe created early in boot) but overflows when the stack is
already deep (benchmark dispatch loop after 42 prior benchmarks).

### Fix

Allocate `PipeInner` directly on the heap via `alloc_zeroed` +
`Box::from_raw`, bypassing the stack entirely:

```rust
#[allow(unsafe_code)]
pub fn new() -> Pipe {
    let inner = unsafe {
        let layout = core::alloc::Layout::new::<PipeInner>();
        let ptr = alloc::alloc::alloc_zeroed(layout) as *mut PipeInner;
        assert!(!ptr.is_null(), "pipe: failed to allocate PipeInner");
        Box::from_raw(ptr)
    };
    // ...
}
```

All fields are correct when zeroed: `rp=0`, `wp=0`, `full=false`,
`closed_by_reader=false`, `closed_by_writer=false`.  The
`MaybeUninit<u8>` ring buffer array doesn't need initialization.

---

## Bug 2: Stale prefault template defeats page cache

### Background

Kevlar pre-maps initramfs pages during `execve` to eliminate demand faults
(each ~500ns under KVM).  The system has two layers:

1. **PAGE_CACHE** — global `HashMap<(file_ptr, page_index), PAddr>` that
   accumulates pages as they're demand-faulted from the initramfs
2. **Prefault template** — cached `Vec<(vaddr, paddr, prot_flags)>` that
   replays page mappings directly, skipping HashMap lookups and VMA
   iteration

The template is an optimization over `prefault_cached_pages` — it turns
O(pages × HashMap lookup) into O(pages × Vec iteration + PTE write).

### The bug

The exec prefault logic:

```rust
if use_template && prefault_template_lookup(file_ptr).is_some() {
    apply_prefault_template(&mut vm, file_ptr);  // Fast path
} else {
    prefault_cached_pages(&mut vm);              // Slow path
    build_and_save_prefault_template(&vm, file_ptr);
}
```

The template is built once (during the first warm-cache exec) and **never
rebuilt**.  But the PAGE_CACHE keeps growing as new code pages are
demand-faulted during subsequent executions.

Trace through the benchmark loop (BusyBox is statically linked, ET_EXEC):

| Step | PAGE_CACHE | Template | Effect |
|------|-----------|----------|--------|
| Iter 1, exec sh | empty | MISS → not saved (empty) | All ~50 ash pages demand-faulted, added to cache |
| Iter 1, exec grep | {ash pages} | MISS → prefault maps ash pages → **saved with ash pages** | grep-specific ~30 pages demand-faulted, added to cache |
| Iter 2, exec sh | {ash + grep} | HIT → maps ash pages | No demand faults for sh ✓ |
| Iter 2, exec grep | {ash + grep} | HIT → maps ash pages only | **grep pages demand-faulted again** ✗ |
| Iter 3+, exec grep | {ash + grep} | HIT → still only ash pages | **grep pages demand-faulted every time** ✗ |

The template captured only the pages that were in PAGE_CACHE *at the time
it was built* (during grep's exec in iteration 1).  Pages demand-faulted
*after* exec (grep-specific code) were added to PAGE_CACHE but never
captured in the template — and the template's existence prevented
`prefault_cached_pages` from running.

**Impact:** ~30-80 unnecessary demand faults per exec at ~300-500ns each
= 10-40µs wasted per exec.  For pipe_grep (2 execs × 100 iterations),
that's 2-8ms of total overhead, explaining the 10-21% regressions.

### Fix

Add a generation counter to PAGE_CACHE that increments on every insertion.
The prefault template stores the generation when it was built.  On
template hit, if the generation has advanced, the template is stale —
fall through to full `prefault_cached_pages` and rebuild:

```rust
// page_fault.rs
pub static PAGE_CACHE_GEN: AtomicU64 = AtomicU64::new(0);

fn page_cache_insert(file_ptr: usize, page_index: usize, paddr: PAddr) {
    // ... insert into cache ...
    PAGE_CACHE_GEN.fetch_add(1, Ordering::Relaxed);
}
```

```rust
// process.rs — PrefaultTemplate now tracks cache generation
struct PrefaultTemplate {
    entries: Vec<(usize, PAddr, i32)>,
    huge_entries: Vec<(usize, PAddr, i32)>,
    cache_gen: u64,
}

// Exec prefault logic:
let current_cache_gen = PAGE_CACHE_GEN.load(Ordering::Relaxed);
if let Some(tpl_gen) = prefault_template_lookup(file_ptr) {
    if tpl_gen == current_cache_gen {
        apply_prefault_template(&mut vm, file_ptr);   // Fresh → fast path
    } else {
        prefault_cached_pages(&mut vm);               // Stale → rebuild
        build_and_save_prefault_template(&vm, file_ptr);
    }
} else {
    prefault_cached_pages(&mut vm);
    build_and_save_prefault_template(&vm, file_ptr);
}
```

After 2-3 iterations, the cache stabilizes (all BusyBox code pages
cached), the generation stops advancing, and the template stays fresh.
All subsequent execs use the fast template path with zero demand faults.

---

## Additional fix: gc_exited_processes double lock

`gc_exited_processes` acquired `EXITED_PROCESSES.lock()` twice — once
for `is_empty()`, once for `clear()`.  Merged into a single critical
section.

---

## Results

Full KVM benchmark comparison (44 benchmarks, fresh Linux baseline):

| Benchmark | Before | After | Linux | Status |
|-----------|--------|-------|-------|--------|
| exec_true | 73-79µs (0.86-0.91x) | 69.1µs (0.80x) | 86.0µs | **Faster** |
| shell_noop | 114-117µs (1.08-1.10x) | 98.8µs (0.93x) | 106.4µs | **Faster** |
| pipe_grep | 357-381µs (1.12-1.20x) | 297.3µs (0.93x) | 318.3µs | **Faster** |
| sed_pipeline | 476-494µs (1.16-1.21x) | 384.6µs (0.94x) | 409.6µs | **Faster** |
| sort_uniq | 1.0-1.1ms (1.00-1.10x) | 855.9µs (0.85x) | 1.0ms | **Faster** |
| tar_extract | 665µs (0.94x) | 549.9µs (0.77x) | 710.1µs | **Faster** |
| sort_uniq/tar_extract | **HANG** | Complete | — | **Fixed** |

Overall: **32 faster, 12 OK, 0 marginal, 0 REGRESSION.**

Contract tests: **116/118 PASS, 2 XFAIL, 0 FAIL** — unchanged.

---

## Files changed

- `kernel/pipe.rs` — `alloc_zeroed` + `Box::from_raw` to bypass 65KB stack allocation
- `kernel/mm/page_fault.rs` — `PAGE_CACHE_GEN` counter, incremented on cache insert
- `kernel/process/process.rs` — `PrefaultTemplate.cache_gen` field, stale-template
  detection in exec prefault, `gc_exited_processes` double-lock fix

## Lessons

1. **Caches need invalidation signals.**  The prefault template was a pure
   optimization (skip HashMap lookups), but without a staleness check it
   silently defeated the page cache it was supposed to accelerate.  A
   monotonic generation counter is the cheapest correct solution — one
   Relaxed atomic load per exec to validate, one Relaxed fetch_add per
   cache insert.

2. **Large inline arrays in Rust are stack-allocated by `Box::new`.**
   `Box::new(T { big_array: [0u8; 65536], .. })` constructs `T` on the
   stack first, then memcpy's to the heap.  With a 16KB kernel stack,
   this is a time bomb.  Use `alloc_zeroed` + `Box::from_raw` for any
   struct larger than ~4KB.

3. **Benchmark suite order matters.**  The pipe hang only manifested as
   benchmark #43 because the dispatch loop's stack frame accumulated
   enough depth to push the 65KB `Box::new` over the edge.  Running
   sort_uniq in isolation passed because the stack was shallow.
