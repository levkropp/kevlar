---
name: Demand fault optimization experiments (2026-03-17)
description: Three failed approaches to beat lazy demand faults under KVM, and why they failed
type: project
---

## Context

shell_noop is ~7-9µs slower than Linux (1.07-1.09x). Root cause: ash startup
hits ~6 more demand page faults than /bin/true. Each fault costs ~2.7µs under
KVM (1.7µs guest handler + 1µs for two VM exits: #PF delivery + EPT population).

## Approach 1: Eager stack prefault (8 pages, then 4 pages)

**Idea:** Allocate and map 4-8 stack pages below the initial SP during exec,
before the process starts. musl/BusyBox startup touches these pages immediately.

**Result:** +5 to +16µs SLOWER (worse than baseline).

**Why it failed:**
- `alloc_pages(1, KERNEL)` bypasses the per-CPU page cache that `alloc_page(USER|DIRTY_OK)` uses
- Even with `alloc_page_batch`, the sequential alloc+zero+map loop is ~800ns/page
- Demand faults use the fast per-CPU page cache (~100ns alloc) and pipeline with userspace execution
- For exec_true, most prefaulted pages are never used (wasted work)
- EPT population VM exit (~500ns) still happens on first access regardless

## Approach 2: CoW-map writable .data pages from page cache

**Idea:** Map .data segment pages as read-only pointing to page cache (like
Linux does). First READ is free (served from cache). First WRITE triggers a
cheap CoW copy instead of a full demand fault.

**Result:** +6µs SLOWER.

**Why it failed:**
- .data pages are NOT in the page cache! The cache excludes writable VMAs
  (line 325-329 in page_fault.rs: `vma_readonly` check in `is_cacheable`)
- All HashMap lookups return None — pure overhead with zero benefit
- Can't safely cache writable pages: same physical page is mapped into process
  AND stored in cache; process writes would corrupt cache for future execs
- Fixing this requires double-allocation (one page for cache, one for process)
  which doubles the cost for ~3 pages of .data — not worth it

## Approach 3: (Not attempted) Pre-populate EPT by kernel-mode page touch

**Idea:** After prefaulting PTEs during exec, touch each mapped page from
kernel mode to force KVM to create EPT entries. Eliminates EPT violation
VM exit on first userspace access.

**Why not attempted:**
- Each kernel touch still causes one VM exit for EPT population (~500ns)
- For 350 text pages at 4KB each: 350 × 500ns = 175µs (catastrophic)
- For 2MB huge page: one touch covers 512 pages → ~500ns total
- But this only saves ONE VM exit (the first code access). The 2MB huge page
  already means all subsequent code accesses hit the same EPT entry.
- Net savings ~500ns for the entire text segment. Not significant.

## Key insight: Why demand faults win under KVM

1. **Per-CPU page cache**: `alloc_page(USER|DIRTY_OK)` is ~100ns (lock-free cache hit).
   Proactive allocation uses `alloc_pages(KERNEL)` which is ~500ns (global lock + bitmap scan).
2. **Pipeline overlap**: Demand faults are interleaved with useful userspace work.
   The CPU's out-of-order engine overlaps fault handling with instruction retirement.
   Proactive allocation serializes all work before the process starts.
3. **Unavoidable EPT cost**: Under KVM, every new page mapping requires an EPT
   violation VM exit (~500ns) on first access, regardless of whether the guest
   PTE was pre-populated. Prefaulting saves the guest #PF VM exit but not the
   EPT VM exit. Net savings: ~500ns/page minus prefault overhead (~800ns/page) = negative.
4. **Waste avoidance**: Demand faults only allocate pages that are actually accessed.
   Prefaulting may allocate pages that are never touched (e.g., extra stack pages
   for /bin/true which exits immediately).

## What DID work

- `/proc/self/exe` → proper symlink (correctness fix, needed for BusyBox standalone)
- BusyBox `FEATURE_SH_STANDALONE` + `FEATURE_SH_NOFORK` (NOFORK applets skip fork/exec)
- Fixed Linux baseline in bench-linux.py (was measuring failed execs, not real workloads)
- Discovered the "4.7x-6.2x workload slowdown" was entirely a measurement artifact
