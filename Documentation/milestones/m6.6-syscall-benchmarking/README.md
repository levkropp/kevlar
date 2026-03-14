# M6.6: Syscall Performance Benchmarking — COMPLETE

## Results

**27/28 benchmarks within 10% of Linux KVM.**  22 faster, 5 at parity,
1 slower (mmap_fault at 1.12x median).

**19/19 contract tests PASS** — zero divergences.

## Phase completion

| Phase | Scope | Status |
|-------|-------|--------|
| 1 | Expand bench.c + baseline | DONE |
| 2 | Fix surface regressions | DONE |
| 3 | Investigate mmap_fault | DONE |
| 4 | Buddy allocator | DONE |
| 5 | Lightweight fault entry + signal fast-path | DONE |
| 6 | Per-CPU page cache (evaluated, reverted) | DONE |
| 7 | Final analysis + documentation | DONE |

## mmap_fault: 12-15% gap, root cause identified

12 optimization attempts exhausted.  Root cause: Rust codegen produces
~40% more instructions than equivalent C in the page fault handler,
causing L1 icache pressure.  Every data-path optimization failed because
L1 data cache already handles repeated traversals optimally.

**Resolution:** Huge page support (M10) eliminates 97% of page faults
for large mappings.  The benchmark is worst-case for small-page demand
paging and does not represent actual GPU driver behavior (which uses
2MB+ allocations).

## Deliverables

- `build/bench-m6.6-final.csv` — benchmark results
- `Documentation/blog/042-m6.6-benchmarks.md` — full analysis
- `Documentation/blog/043-mmap-fault-investigation.md` — 12 attempts documented
- Buddy allocator (`libs/kevlar_utils/buddy_alloc.rs`)
- setitimer/alarm implementation
- tkill syscall
- 19/19 contract tests passing
