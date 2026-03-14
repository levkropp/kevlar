# M6.6: Syscall Performance Benchmarking

## Goal

Establish a performance baseline for all implemented syscalls before M7
adds /proc (which touches VFS paths on every open/stat/read).  Every
syscall must be within 10% of Linux on KVM.  Any regression found is
fixed before proceeding.

## Why now

- M5 benchmarked 8 core syscalls and found 65x speedups, but M6 added
  SMP, threading, and M6.5 added getpriority/mprotect/brk changes —
  none benchmarked yet.
- M7 will add /proc filesystem lookups in the VFS hot path.  We need a
  clean baseline so we can detect /proc-induced regressions.
- The benchmark infrastructure (`benchmarks/bench.c`,
  `tools/run-all-benchmarks.py`, `make bench-kvm`) already exists.

## Scope

- Run all 24 existing benchmarks (8 core + 16 extended) on KVM
- Add 4 new benchmarks for syscalls added since M5
- Compare to Linux KVM baseline
- Fix any syscall >10% slower than Linux
- Publish results table in blog post

## Phases

| Phase | Scope | Duration |
|-------|-------|----------|
| 1 | Expand bench.c + baseline both kernels | 1 day |
| 2 | Analyze results, fix regressions >10% | 1-2 days |
| 3 | Final benchmark run + results publication | 0.5 day |
| **Total** | | **~3 days** |

## Success Criteria

- All 28 benchmarks run on both Linux and Kevlar under KVM
- No syscall >10% slower than Linux on KVM
- Results published as CSV + blog post
- M6 test suites still pass (no correctness regressions from perf fixes)

## Non-goals

- TCG performance (too noisy, not representative)
- ARM64 benchmarks (no KVM available in our CI)
- Application-level benchmarks (that's M10 scope)
