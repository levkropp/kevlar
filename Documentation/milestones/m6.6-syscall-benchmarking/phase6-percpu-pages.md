# M6.6 Phase 6: Per-CPU Page Free Lists

**Duration:** ~1 day
**Prerequisite:** Phase 4 (buddy allocator)
**Goal:** Lock-free single-page allocation via per-CPU free lists.

## Current design

```
alloc_page() → PAGE_CACHE.lock_no_irq().pop()   [global, locked]
refill      → ZONES.lock_no_irq() → bitmap scan [global, locked]
```

With the buddy allocator from Phase 4, refill becomes O(1) per page.
But the global `PAGE_CACHE` lock is still acquired for every alloc,
even on a single CPU.  While `lock_no_irq()` is cheap (~5ns), it's
not zero — and it's unnecessary when we have per-CPU data.

## Design

### Per-CPU page cache

```rust
const PER_CPU_CACHE_SIZE: usize = 32;
const MAX_CPUS: usize = 16;

struct CpuPageCache {
    pages: [usize; PER_CPU_CACHE_SIZE],
    count: usize,
}

// Accessed with preempt_disable — no lock needed.
// Safety: each CPU only touches its own entry.
static mut CPU_CACHES: [CpuPageCache; MAX_CPUS] = ...;
```

### alloc_page fast path

```rust
pub fn alloc_page(flags: AllocPageFlags) -> Result<PAddr, PageAllocError> {
    // Per-CPU fast path: preempt guard, no lock
    arch::preempt_disable();
    let cpu = arch::cpu_id() as usize;
    let cache = unsafe { &mut CPU_CACHES[cpu] };
    if let Some(paddr) = cache.pop() {
        arch::preempt_enable();
        if !flags.contains(AllocPageFlags::DIRTY_OK) {
            zero_page_internal(paddr);
        }
        NUM_FREE_PAGES.fetch_sub(1, Ordering::Relaxed);
        return Ok(paddr);
    }
    arch::preempt_enable();

    // Slow path: refill from buddy allocator under global lock
    refill_cpu_cache(cpu);
    // ... retry per-CPU cache
}
```

### free_pages fast path

```rust
pub fn free_pages(paddr: PAddr, num_pages: usize) {
    if num_pages == 1 {
        arch::preempt_disable();
        let cpu = arch::cpu_id() as usize;
        let cache = unsafe { &mut CPU_CACHES[cpu] };
        if cache.push(paddr) {
            arch::preempt_enable();
            NUM_FREE_PAGES.fetch_add(1, Ordering::Relaxed);
            return;
        }
        arch::preempt_enable();
        // Cache full — drain half to buddy allocator, then push
    }
    // Multi-page free goes directly to buddy allocator
}
```

### Refill and drain

- **Refill**: lock buddy allocator, pop 32 pages, push to per-CPU cache
- **Drain**: when per-CPU cache is full (32 entries), free 16 pages
  back to buddy allocator to keep the cache at ~50% capacity
- **Cross-CPU free**: page freed on CPU X goes to CPU X's cache.
  No special handling needed — the buddy allocator handles the physical
  addresses regardless of which CPU allocated them.

## Why preempt_disable is sufficient

- Timer interrupts don't allocate pages
- Page faults can't nest (double fault → panic)
- No other interrupt handler calls alloc_page
- preempt_disable prevents context switches, so we stay on the same CPU

## Expected impact

- Eliminates ~5ns per alloc (lock_no_irq overhead)
- With 17 allocs per fault: ~85ns per fault
- More importantly: reduces code path length in the hot path,
  which improves icache behavior
- Per page amortized: ~5ns savings

Combined with Phase 4 (buddy: ~6ns/page) and Phase 5 (fault entry:
~3.4ns/page), total expected savings: ~14ns/page.

Over 4096 pages: ~57µs.  From current 7.7ms to ~7.64ms.  That's still
only ~1% improvement — but the structural improvements from the buddy
allocator should have a larger second-order effect (fewer cache misses,
shorter code paths, less lock contention under SMP).

## Files to modify

- `platform/page_allocator.rs` — add per-CPU cache, modify alloc/free
- `platform/x64/mod.rs` — verify preempt_disable/enable are inline
- `platform/arm64/mod.rs` — same verification

## Testing

- `make test-threads-smp` — 14/14 (multi-CPU allocation correctness)
- `make test-contracts-vm` — 8/8 (demand paging)
- `make bench-kvm` — mmap_fault target: within 10% of Linux KVM
