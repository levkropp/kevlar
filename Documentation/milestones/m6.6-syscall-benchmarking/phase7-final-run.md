# M6.6 Phase 7: Final Benchmark Run

**Duration:** ~0.5 day
**Prerequisite:** Phases 4-6
**Goal:** Verify all 28 benchmarks within 10% of Linux KVM, publish final results.

## Procedure

1. Clean build: `make clean && make build PROFILE=balanced`
2. CPU-pinned Kevlar KVM: `taskset -c 0 make bench-kvm` (3 runs, best)
3. CPU-pinned Linux KVM: `taskset -c 0 qemu ... -mem-prealloc` (3 runs, best)
4. Run all 4 profiles: balanced, performance, ludicrous, fortress
5. Save to `build/bench-m6.6-final.csv`
6. Update blog post (042-m6.6-benchmarks.md)

## Success criteria

- [ ] All 28 benchmarks: Kevlar/Linux ratio ≤ 1.10
- [ ] All 4 profiles equivalent (no profile-specific regressions)
- [ ] `make test-contracts` — 18/19 PASS
- [ ] `make test-threads-smp` — 14/14 PASS
- [ ] `make test-regression-smp` — 15/15 PASS
- [ ] Blog post updated with final results table
- [ ] Results CSV committed

## If mmap_fault is still >10%

If the buddy allocator + lightweight fault entry + per-CPU lists
don't fully close the gap, document the remaining percentage and
the exact cause.  Possible additional measures:

- Profile-guided optimization (PGO) for the kernel binary
- Assembly page fault handler for the hot path (bypass Rust ISR)
- Reduced fault-around (8 instead of 16) if the overhead per-fault
  exceeds the savings from fewer faults
- Accept and document a ≤12% gap with clear justification for why
  it doesn't affect real-world gaming (page faults are rare during
  steady-state rendering)
