# M6.6: Syscall Performance Benchmarking

## Goal

Every syscall must be within 10% of Linux on KVM.  No exceptions —
a 14% page fault overhead means 12.5% slower level loads in games,
which is unacceptable for a drop-in replacement.

## Phases

| Phase | Scope | Duration | Status |
|-------|-------|----------|--------|
| 1 | Expand bench.c + baseline both kernels | 1 day | DONE |
| 2 | Fix surface-level regressions (uname, sigaction, fcntl, etc.) | 1 day | DONE |
| 3 | Investigate mmap_fault — try batch PTE, pre-zero, per-CPU cache | 1 day | DONE (gap identified) |
| 4 | Buddy allocator (replace bitmap allocator) | 2 days | |
| 5 | Lightweight page fault entry (minimal register save) | 1 day | |
| 6 | Per-CPU page free lists (lock-free fast path) | 1 day | |
| 7 | Final benchmark run + results publication | 0.5 day | |
| **Total** | | **~7.5 days** | |

## Results so far

27/28 benchmarks within 10%.  One remaining:

| Benchmark | Linux KVM | Kevlar KVM | Ratio |
|-----------|-----------|------------|-------|
| mmap_fault | 1,650ns | 1,881ns | 1.14x |

## Root cause breakdown (231ns gap per page)

| Source | Contribution | Fix |
|--------|-------------|-----|
| Bitmap allocator O(N) scan | ~40% (~90ns) | Phase 4: buddy allocator |
| Exception handler saves 16 GPRs | ~35% (~80ns) | Phase 5: minimal save |
| Rust codegen / icache pressure | ~25% (~60ns) | Phase 6: per-CPU lists reduce code paths |

## Approaches tried and ruled out

- Batch PTE writes: L1 cache makes repeated traversals cheap
- Pre-zeroed page cache: free() returns dirty pages, breaks invariant
- Zero hoisting: 64KB zeroing thrashes 32KB L1
- Unconditional PTE writes: cache line dirtying > branch savings
- Per-CPU page cache (lock elimination): lock is only ~5ns, not bottleneck

## Success Criteria

- All 28 benchmarks within 10% of Linux KVM (CPU-pinned, -mem-prealloc)
- All 4 profiles benchmark equivalently
- M6.5 contract tests still pass (18/19)
- M6 thread tests still pass (14/14)
