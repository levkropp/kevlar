# M9.6: Optimization Pass — Linux KVM Parity

**Goal:** Eliminate all benchmark regressions vs Linux on KVM, then
unblock Alpine integration test layers 3-7.

## Current state (post-070)

**Micro-benchmarks (42 syscalls):** 19 faster, 14 parity, 5 marginal, 4 regressions
**BusyBox benchmarks (22 workloads):** 21 faster, 1 regression (bb_dd 1.4x)

### Regressions to fix

| Benchmark | Kevlar | Linux | Ratio | Root cause area |
|-----------|-------:|------:|------:|-----------------|
| sed_pipeline | 1.37ms | 64µs | **21.4x** | tmpfs read for large files |
| pipe_grep | 979µs | 65µs | **15.1x** | tmpfs read for large files |
| shell_noop | 345µs | 64µs | **5.4x** | exec path / ELF loading |
| exec_true | 177µs | 67µs | **2.6x** | fork+exec+wait overhead |
| bb_dd | 6.7ms | 4.9ms | **1.4x** | tmpfs write (Vec realloc) |
| read_null | 158ns | 132ns | **1.2x** | /dev/null read overhead |
| write_null | 158ns | 132ns | **1.2x** | /dev/null write overhead |
| sched_yield | 219ns | 196ns | **1.1x** | scheduler yield path |
| epoll_wait | 150ns | 136ns | **1.1x** | event notification |
| eventfd | 386ns | 349ns | **1.1x** | event notification |

### Missing benchmarks

`sort_uniq` and `tar_extract` don't run on Kevlar (likely crash or
missing dependency). Need investigation.

## Phase completion

| Phase | Scope | Status |
|-------|-------|--------|
| 1 | [tmpfs read path](phase1-tmpfs-read.md) (pipe_grep 15x, sed_pipeline 21x) | TODO |
| 2 | [exec path](phase2-exec-path.md) (exec_true 2.6x, shell_noop 5.4x) | TODO |
| 3 | [micro-regressions](phase3-micro-regressions.md) (null, yield, epoll, eventfd) | TODO |
| 4 | [tmpfs write](phase4-tmpfs-write.md) (bb_dd 1.4x) | TODO |
| 5 | [Alpine integration](phase5-alpine-integration.md) (layers 3-7) | TODO |

## Success criteria

- All micro-benchmarks within 1.1x of Linux KVM (no regressions)
- All BusyBox benchmarks within 1.1x of Linux KVM
- `sort_uniq` and `tar_extract` benchmarks running
- Alpine test layers 3-7 passing
