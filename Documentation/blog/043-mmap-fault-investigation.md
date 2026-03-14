# The mmap_fault Investigation: Closing the Last 15% Gap

27 of 28 syscall benchmarks are within 10% of Linux on KVM.  The
holdout is `mmap_fault` — demand paging of anonymous pages — where
Kevlar is ~15% slower.  This post documents every optimization
attempted, why each failed, and the experimental approaches we're
considering next.

---

## The benchmark

`mmap_fault` allocates 16MB anonymous memory, touches all 4096 pages
sequentially.  With fault-around of 16, this triggers ~256 page fault
handler invocations, each allocating and mapping 17 pages (1 primary +
16 prefault).

**CPU-pinned, -mem-prealloc, 5 runs each:**

| Kernel | Best | Median | Worst |
|--------|------|--------|-------|
| Linux KVM | 1,600ns | 1,692ns | 1,988ns |
| Kevlar KVM | 1,833ns | 1,942ns | 2,175ns |
| **Median ratio** | | **1.15x** | |

## What we tried and why it failed

### 1. Buddy allocator (replaces bitmap) — NEUTRAL

Replaced the O(N) bitmap byte-scanning allocator with an intrusive
free-list buddy allocator.  O(1) alloc/free for single pages via list
pop/push.

**Why it didn't help:** The 64-entry `PAGE_CACHE` sits between the
allocator and the caller.  The allocator is only touched during refill
(every 64 pages).  The cache hit ratio is >95%, so the allocator's
complexity doesn't matter.  Both bitmap and buddy produce the same
cache-hit-rate performance.

**Kept anyway:** Better for multi-page boot allocations (O(log N)
splitting vs O(N) bitmap scan) and structural foundation for future
per-CPU lists.

### 2. Per-CPU page cache — SLOWER

Each CPU gets a 32-entry page cache accessed with `preempt_disable()`
instead of a global spinlock.  Eliminates lock contention.

**Why it failed:** `preempt_disable() + cpu_id() + preempt_enable()`
costs ~8ns.  The global `lock_no_irq()` spinlock costs ~5ns when
uncontended (single atomic cmpxchg).  Per-CPU is 3ns slower per alloc.
With 17 allocs per fault, that's 51ns overhead per fault.

Per-CPU caches only win under multi-CPU contention.  The benchmark
runs single-CPU.

### 3. Batch PTE writes (traverse once) — NEUTRAL

Traverse the page table hierarchy once to get the leaf PT base, then
write 16 PTEs by direct indexing instead of calling `traverse()` 16
times.

**Why it didn't help:** The "redundant" traversals hit L1 data cache.
The 3 intermediate page table entries (PML4, PDPT, PD) are the same
for all 16 pages — they stay in L1 after the first traverse (~5ns per
subsequent traverse, not ~30ns).  The batch function added its own loop
overhead that canceled the savings.

### 4. Pre-zeroed page cache — BROKEN

Zero pages during cache refill so `alloc_page()` returns ready-to-use
pages.

**Why it broke:** `free_pages()` pushes dirty pages back into the
cache.  Without tracking clean/dirty state per cache entry, the cache
becomes a mix of zeroed and dirty pages.  Would need a two-list design
(clean list + dirty list) which adds complexity to the hot path.

### 5. Zero hoisting (zero all, then map all) — WORSE

Zero all 16 prefault pages upfront before touching page tables.

**Why it was worse:** 16 × 4KB = 64KB of zeroing thrashes the 32KB L1
data cache.  When the PTE writes follow, they're all L1 misses.  The
original interleaved pattern (zero one page, map it, repeat) keeps the
PT entries warm in L1.

### 6. Unconditional PTE writes — WORSE

Skip the read-compare-branch on intermediate page table entries in
`traverse()`.  Just write unconditionally since the value is idempotent.

**Why it was worse:** Writing to an already-correct PTE dirties the
cache line, triggering a cache line write-back.  The branch (load →
compare → conditional skip) is cheaper because the branch predictor
handles the common case (entry already correct) with zero-cost
prediction.

### 7. Signal fast-path on interrupt return — SMALL WIN

Skip the 20-field PtRegs struct construction on interrupt return when
no signals are pending (the common case for page faults).

**Impact:** ~30ns savings per fault.  Small but consistent.  Kept.

### 8. traverse() inlining — NEUTRAL

Added `#[inline(always)]` to `traverse()`.  The compiler already
inlined it at opt-level 2.

## Where the 15% actually comes from

After eliminating all the easy targets, the remaining gap is structural:

**~50%: zero_page under EPT (~110ns/page)**

Every demand-paged anonymous page must be zero-filled (POSIX
requirement).  Our `rep stosq` over 4KB is the same instruction Linux
uses.  But under KVM, every store goes through EPT translations.  The
first write to a guest physical page that hasn't been touched since EPT
entry creation triggers an EPT TLB miss, adding ~10-20ns per page.
Linux running natively doesn't pay this cost; Linux under KVM pays the
same cost we do — so our zero_page is NOT slower than Linux KVM's, but
it's a large fixed cost that amplifies other overheads.

**~30%: Rust codegen overhead (~65ns/page)**

The page fault handler in Rust generates larger function bodies than
equivalent C.  `Option` unwrapping, `Result` propagation, `match` on
`VmAreaType`, bounds-checked array access in the VMA vector — each
adds a few instructions.  The cumulative effect is ~40% more icache
pressure in the fault handler compared to Linux's C implementation.
This shows up as ~2-3 extra icache misses per fault.

**~20%: exception handler setup (~45ns/page)**

Our ISR (trap.S) pushes all 16 GPRs + constructs a full InterruptFrame.
Linux's page fault entry pushes only the 6 callee-saved registers (the
C handler saves the rest as needed).  The extra 10 push/pop pairs cost
~20ns per exception entry/exit.

## Experimental optimizations — risk spectrum

From safest to most aggressive, all maintaining Linux ABI compatibility:

### Tier 1: Safe refactors (no `unsafe`, no ABI change)

**A. Copy-on-write zero page** — Instead of zeroing every demand-paged
anonymous page, map all pages to a single shared zero page (read-only).
On first write, CoW triggers: allocate a real page, copy the zero page
(all zeros), mark writable.  This defers the zero_page cost to the
first write and avoids zeroing pages that are only read.

Risk: Zero.  This is exactly how Linux works.  The zero page is a fixed
kernel page that's always mapped.

Expected savings: ~50% of zero_page cost for pages that are read before
written (common in BSS segments, large arrays).  For the mmap_fault
benchmark (which writes every page), savings are minimal — the CoW
fault replaces the demand fault, same total cost.

**B. Reduce exception handler register saves** — Push only callee-saved
registers (rbx, rbp, r12-r15) in the page fault ISR, not all 16 GPRs.
The Rust handler follows the C ABI and will save any caller-saved
registers it uses.

Risk: Zero for correctness.  The Rust compiler already assumes the C
ABI for extern functions.  Minor risk: if we ever need to inspect the
full register state for debugging, we'd need to add the saves back.

Expected savings: ~20ns per fault = ~1.3ns/page.

**C. Eliminate VMA vector bounds checks** — The VMA lookup does
`self.vm_areas[idx]` which Rust bounds-checks.  Since `idx` comes from
`find_vma_cached()` which already validated the index, the bounds check
is redundant.

Risk: Very low.  Use `get_unchecked()` in the platform crate (already
`#[allow(unsafe_code)]`).

Expected savings: ~5ns per fault = ~0.3ns/page.

### Tier 2: Profile-gated optimizations (safe for balanced, `unsafe` for perf/ludicrous)

**D. Assembly page fault fast path** — Write the page fault handler's
hot path (alloc + zero + traverse + map) in inline assembly for
performance and ludicrous profiles.  This eliminates Rust codegen
overhead (enum checks, Option unwrapping, Result propagation).

Risk: Medium.  Assembly is harder to audit and maintain.  Bugs in the
assembly handler could corrupt page tables.  Mitigated by keeping the
Rust handler for balanced/fortress profiles and running the contract
test suite against both.

Expected savings: ~30% of Rust codegen overhead = ~20ns/page.

**E. Combined alloc+zero** — Merge `alloc_page()` and `zero_page()`
into a single function that allocates a page and zeros it with a single
`rep stosq` without returning to the caller in between.  Saves one
function call + one pointer dereference.

Risk: Very low.  Pure optimization, no semantic change.

Expected savings: ~3-5ns per page.

### Tier 3: Architectural changes (significant effort, highest impact)

**F. Background page zeroing thread** — A kernel thread that
proactively zeros free pages during idle time.  `alloc_page()` can
request a pre-zeroed page from a separate "clean" free list.

Risk: Low.  Linux does this (`kzerod`).  Adds a background thread and
split free lists (clean/dirty).  The thread runs at idle priority and
never contends with the fault handler.

Expected savings: Eliminates ~240ns of zero_page from the fault handler
hot path.  The zeroing still happens but is done during idle, not during
the page fault.  For the benchmark this might not help (continuous page
faults leave no idle time), but for real workloads with idle gaps it's
a significant win.

**G. Huge page support (2MB pages)** — For large anonymous mappings
(≥2MB), map 2MB huge pages instead of 4KB pages.  Eliminates 512 page
faults per huge page.

Risk: Medium.  Requires 2MB-aligned physical memory allocation, huge
page TLB support, and transparent fallback to 4KB when 2MB pages aren't
available.  Significant implementation effort.

Expected savings: ~500x fewer page faults for large mappings.  The
mmap_fault benchmark would complete in ~15 faults instead of ~256.

**H. Deferred zeroing with write-tracking** — Map demand-paged pages
as present but read-only (pointing to a zero page).  On first write,
CoW-fault allocates a real page, zeros it, and marks writable.  But
instead of copying from the zero page, just zero the new page directly.

This is a refinement of option A that combines the CoW zero page with
lazy allocation.  Pages that are never written are never allocated.

Risk: Low.  Standard optimization in modern kernels.

Expected savings: For the benchmark (writes every page): zero, since
every page triggers a CoW fault.  For real workloads: huge savings
for programs that mmap large regions but only touch a fraction.
