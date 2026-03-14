# M6.6 Phase 3: Final Run and Results Publication

**Duration:** ~0.5 day
**Prerequisite:** Phase 2 fixes applied
**Goal:** Produce the definitive benchmark comparison and blog post.

## Final benchmark run

1. Clean build: `make clean && make build PROFILE=balanced`
2. Run Kevlar: `make bench-kvm` (3 runs, take median)
3. Run Linux: `python3 tools/run-all-benchmarks.py --linux --kvm` (3 runs, take median)
4. Save to `build/bench-m6.6-final.csv`

## Results table format

```markdown
| Benchmark | Linux (ns) | Kevlar (ns) | Ratio | Status |
|-----------|-----------|-------------|-------|--------|
| getpid | 338 | 200 | 0.59x | FASTER |
| read_null | 570 | 514 | 0.90x | FASTER |
| fork_exit | 85000 | 82000 | 0.96x | OK |
| mprotect | 1200 | 1300 | 1.08x | OK |
| ... | ... | ... | ... | ... |
```

Status: FASTER (<0.95), OK (0.95-1.10), SLOW (>1.10)

## Blog post outline

1. **Why benchmark now** — M5 established initial performance, M6/M6.5
   changed kernel internals, M7 will change VFS paths
2. **Methodology** — KVM, -mem-prealloc, 3 runs median, same QEMU version
3. **Results table** — all 28 benchmarks
4. **Highlights** — any particularly interesting results
5. **Fixes applied** — summary of Phase 2 changes
6. **Baseline for M7** — these numbers become the regression target

## Success criteria

- [ ] All 28 benchmarks complete on both kernels
- [ ] No benchmark >10% slower than Linux
- [ ] Results CSV committed to `build/`
- [ ] Blog post published
- [ ] `make test-contracts` passes (18/19)
- [ ] `make test-threads-smp` passes (14/14)
