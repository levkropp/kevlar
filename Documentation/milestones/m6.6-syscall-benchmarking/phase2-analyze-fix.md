# M6.6 Phase 2: Analyze Results and Fix Regressions

**Duration:** 1-2 days
**Prerequisite:** Phase 1 baseline data
**Goal:** Identify any syscall >10% slower than Linux on KVM and fix it.

## Analysis procedure

1. Load `build/bench-m6.6-baseline.csv`
2. For each benchmark, compute ratio: `kevlar_ns / linux_ns`
3. Flag any ratio > 1.10 (Kevlar >10% slower)
4. Sort by ratio descending (worst regressions first)

## Likely regression sources

Based on M5 experience, performance issues come from:

### Lock contention
- **Symptom:** syscalls that touch shared state (fd table, VFS) are slow
- **Fix:** `lock_no_irq()` for locks not accessed from IRQ context
- **Example from M5:** fd table lock was using full `cli/sti` pair; switching
  to `lock_no_irq()` saved ~2µs per syscall

### Unnecessary allocations
- **Symptom:** syscalls that should be zero-alloc are slow
- **Fix:** stack-allocate path buffers, avoid Vec/String in hot paths
- **Example:** stat() creating a PathBuf per call

### Redundant usercopy
- **Symptom:** syscalls that copy data to/from userspace multiple times
- **Fix:** batch copies, write structs in one shot
- **Example from M5:** clock_gettime wrote tv_sec and tv_nsec separately;
  combined into single Timespec write

### Debug instrumentation
- **Symptom:** all syscalls uniformly slower
- **Fix:** ensure `debug::emit()` compiles to nothing when debug is off
- **Check:** `KEVLAR_DEBUG` should be empty for benchmark builds

## Fix methodology

For each regression:
1. Profile with `KEVLAR_DEBUG=profile` to identify hot function
2. Check if the issue is lock, alloc, copy, or algorithmic
3. Apply minimal fix
4. Re-run single benchmark to verify improvement
5. Run `make test-contracts` to verify no correctness regression

## Targets by category

| Category | Target | Notes |
|----------|--------|-------|
| Trivial (getpid, gettid) | <300ns | Dominated by VMCALL overhead |
| FD ops (read, write, dup) | <600ns | One fd table lookup |
| Path ops (open, stat, access) | <2µs | VFS path traversal |
| Memory (mmap, mprotect, brk) | <3µs | Page table manipulation |
| Process (fork, signal) | <50µs | Heavy operations, just match Linux |
| Pipe throughput | >500 MB/s | Memory copy bound |

## Deliverables

- Updated `build/bench-m6.6-baseline.csv` with post-fix numbers
- List of fixes applied (commit messages)
- No `make test-contracts` regressions
