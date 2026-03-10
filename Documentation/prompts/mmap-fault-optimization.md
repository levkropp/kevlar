# Prompt: Close the mmap_fault Performance Gap

## Context

Kevlar is a Rust kernel that runs Linux binaries. We've optimized all core
syscall benchmarks to beat native Linux (getpid 63ns vs 97ns, stat 262ns
vs 389ns, etc.), but `mmap_fault` — anonymous demand paging throughput —
remains 1.81x behind Linux KVM:

```
Linux Native:  1,047 ns/fault
Linux KVM:     2,104 ns/fault  (2x overhead from EPT violations)
Kevlar KVM:    3,808 ns/fault  (1.81x behind Linux KVM)
```

The goal is to close the 1,704ns gap between Kevlar KVM and Linux KVM,
getting mmap_fault to within 10% of Linux KVM (target: <2,314 ns/fault).

## What mmap_fault measures

`benchmarks/bench.c` `bench_mmap_fault()`: calls `mmap(NULL, 16MB,
PROT_READ|PROT_WRITE, MAP_ANONYMOUS|MAP_PRIVATE, -1, 0)`, then touches
each of 4096 pages sequentially (`p[i * 4096] = (char)i`). Timer starts
before mmap, ends after all touches. Per-iteration = (mmap + 4096 faults)
/ 4096.

## The demand paging hot path

Each page fault follows this path (read these files):

1. **Exception entry** (`platform/x64/trap.S`): `cli`, push all 15 GPRs,
   `swapgs` if from usermode, call `x64_handle_interrupt`

2. **Rust dispatcher** (`platform/x64/interrupt.rs`): reads CR2, validates
   user address, calls `handle_page_fault()`

3. **Page fault handler** (`kernel/mm/page_fault.rs`):
   - `vm.lock()` — SpinLock with cli/sti (LOCK #1)
   - Linear scan `vm.vm_areas().iter().find()` for matching VMA (O(n))
   - `alloc_pages(1, USER | DIRTY_OK)` — see below (LOCK #2 + LOCK #3)
   - `zero_page(paddr)` — `rep stosq` 4KB
   - `vm.page_table_mut().map_user_page_with_prot()` — walk 4-level page
     table via `traverse()`, write PTE

4. **Page allocator** (`platform/page_allocator.rs`):
   - `ZONES.lock()` — SpinLock with cli/sti (LOCK #2)
   - Iterates zones, calls `zone.alloc_pages(order)`
   - `zone.alloc_pages()` acquires `self.bitmap.lock()` (LOCK #3) — a
     `spin::Mutex` inside the `BitMapAllocator`
   - Bitmap scan: `first_zero()` then `not_any()` — O(n) linear bit scan

5. **Page table traverse** (`platform/x64/paging.rs:48-85`):
   - Walks PML4 → PDPT → PD → PT (3 memory loads, potential cache misses)
   - Allocates intermediate tables if needed (rare after first few faults)
   - **Bug**: line 76 unconditionally writes every PTE entry even when
     unchanged (causes unnecessary cache line invalidation)

6. **Exception exit** (`platform/x64/trap.S`): pop all GPRs, `cli` +
   `swapgs` if returning to user, `iretq`

## Identified bottlenecks (ranked by estimated impact)

### 1. Three nested spinlocks per fault (~200-400ns)
- VM lock (`kernel/mm/page_fault.rs:92`) — `lock()` with cli/sti
- ZONES lock (`platform/page_allocator.rs:94`) — `lock()` with cli/sti
- Bitmap inner lock (`libs/kevlar_utils/bitmap_allocator.rs:46`) — `spin::Mutex`

The VM lock is held for the ENTIRE fault duration. The ZONES lock is held
while zeroing pages (when not using DIRTY_OK). Three cli/sti pairs.

**Fix ideas**:
- Allocate page BEFORE acquiring VM lock (alloc doesn't need VM state)
- Change ZONES lock to `lock_no_irq()` if safe (check: is alloc_pages ever
  called from interrupt context? Network RX? Timer?)
- Remove the inner `spin::Mutex` from BitMapAllocator — it's redundant
  since ZONES lock already serializes access
- Or better: per-CPU single-page free list that requires no lock at all

### 2. Bitmap allocator is O(n) per allocation (~100-300ns)
`libs/kevlar_utils/bitmap_allocator.rs` does linear bit scanning via
bitvec's `first_zero()`. For single-page allocs (the common case), this
scans from the beginning every time.

**Fix ideas**:
- Add a `next_free` hint that remembers where the last allocation was
  (roving pointer / next-fit). Avoids re-scanning already-allocated region.
- Replace with a proper buddy allocator (there's a TODO on line 13 of
  `page_allocator.rs` noting bugs in the previous buddy implementation).
  The `buddy_system_allocator` crate v0.11 is already in dependencies.
- Or: a simple free-list for order-0 (single page) allocations. Push freed
  pages onto a stack, pop on alloc. O(1) with zero scanning.

### 3. Redundant PTE writes in traverse() (~30-50ns)
`platform/x64/paging.rs:76`: `*entry = table_paddr.value() | attrs.bits()`
runs on every level even when the entry already has the right value. This
dirties cache lines unnecessarily.

**Fix**: Only write if the value changed:
```rust
let new_val = table_paddr.value() as u64 | attrs.bits();
if unsafe { *entry } != new_val {
    unsafe { *entry = new_val };
}
```

### 4. VMA lookup is linear scan (~50-100ns, low priority)
Only matters with many VMAs. The benchmark has ~3-4 VMAs so this is minor.
But for correctness and future-proofing, consider a sorted Vec with binary
search, or a simple "last VMA" cache.

### 5. Page zeroing (~40-80ns, irreducible)
`rep stosq` for 4KB is already hardware-optimized. Linux doesn't avoid
this cost either (except with pre-zeroed page pools). Not much to gain
here, but a pre-zeroed page cache could amortize the cost.

## Optimization strategy (suggested order)

**Phase 1: Low-hanging fruit (target: ~500-800ns savings)**
1. Reorder fault handler: alloc page + zero BEFORE acquiring VM lock
2. Remove redundant inner spin::Mutex from BitMapAllocator
3. Fix unconditional PTE writes in traverse()
4. Change page allocator ZONES lock to lock_no_irq (verify safety first)

**Phase 2: Allocator improvement (target: ~300-500ns savings)**
5. Add next-fit hint to bitmap allocator (simple, big impact)
6. OR: free-list for order-0 pages (stack-based, O(1) alloc/free)
7. OR: fix the buddy allocator bugs and switch to it

**Phase 3: Architectural (target: ~200-400ns savings)**
8. Per-CPU page cache (no-lock fast path for single page alloc)
9. Split VM lock: separate lock for page table vs VMA list
10. Last-VMA cache in page fault handler

## How to benchmark

```bash
# Build and run Kevlar benchmarks (auto-detects KVM)
python3 benchmarks/run-benchmarks.py run

# Run Linux in KVM for apples-to-apples comparison
gcc -static -O2 -o /tmp/bench benchmarks/bench.c
# Then boot /tmp/bench in a minimal QEMU/KVM Linux VM (see blog 017)

# Run Linux natively for reference
/tmp/bench
```

Target: `mmap_fault` per_iter_ns < 2,314 (within 10% of Linux KVM's 2,104).

## Key files to modify

- `kernel/mm/page_fault.rs` — reorder alloc vs lock, main handler
- `platform/page_allocator.rs` — ZONES lock, alloc_pages interface
- `libs/kevlar_utils/bitmap_allocator.rs` — O(n) scan, inner lock
- `platform/x64/paging.rs` — traverse() PTE writes, map_user_page_with_prot
- `kernel/mm/vm.rs` — VMA data structure (if upgrading to binary search)

## Constraints

- Single-CPU only (no SMP yet), so "per-CPU" just means "lock-free global"
- No SSE in kernel (`+soft-float` target), so no SIMD for zeroing
- `#![deny(unsafe_code)]` on the kernel crate; unsafe only in platform/
- Must not regress other benchmarks (getpid, stat, pipe, etc.)
- The `make check` type-check must pass; `make run` must boot BusyBox shell
