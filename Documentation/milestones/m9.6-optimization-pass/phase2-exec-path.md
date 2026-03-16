# M9.6 Phase 2: Exec Path Optimization

**Regressions:** `exec_true` (2.6x), `shell_noop` (5.4x)
**Target:** Both within 1.1x of Linux KVM

## The problem

`exec_true` does `fork + execl("/bin/true") + waitpid` in a loop.
Linux KVM: 67µs per iteration.  Kevlar: 177µs — 2.6x slower.

`shell_noop` does `fork + execl("/bin/sh", "-c", "true") + waitpid`.
Linux: 64µs.  Kevlar: 345µs — 5.4x slower.  This is roughly 2x
`exec_true` (two execs: sh then true), so the per-exec overhead is
consistent at ~170µs.

The raw `fork_exit` benchmark (fork + _exit + waitpid, no exec) is
0.89x — **faster** than Linux (54µs vs 60µs).  So the regression is
entirely in `execve`, not fork or wait.

## Analysis approach

1. **Profile execve** — use `KEVLAR_DEBUG=profile` to measure time
   spent in each phase of execve:
   - ELF header parsing (read from initramfs)
   - Address space setup (mmap text, data, bss, stack, vDSO)
   - Page table construction
   - Demand paging of first text pages
   - Auxiliary vector + argument copying

2. **Compare initramfs read latency** — BusyBox ELF is read from
   initramfs (memory-backed).  Profile how long the actual ELF
   loading takes vs address space manipulation.

3. **Check de_thread overhead** — execve calls `de_thread()` to kill
   sibling threads.  For single-threaded processes this should be a
   no-op, but verify it's not doing unnecessary work.

4. **Measure TLB flush cost** — execve tears down the old address
   space and builds a new one.  On KVM, this involves EPT invalidation
   which is expensive.

## Potential fixes

### Fix A: batch page table operations in execve

Currently execve does individual `mmap` calls for each ELF segment,
then sets up the stack and vDSO.  Each mmap involves page table
manipulation.  Batch these into a single address space construction
pass.

### Fix B: pre-fault text pages

The first instruction fetch after execve triggers a demand page fault.
If we pre-fault the first few text pages during execve (before jumping
to user space), we avoid the fault overhead on the hot path.

### Fix C: optimize address space teardown

The old address space is torn down during execve.  If we can defer
page table page freeing or batch TLB flushes, this saves VM exit
overhead on KVM.

### Fix D: fast-path single-threaded de_thread

If `thread_group_size == 1`, de_thread can skip all synchronization
and thread-killing logic.

## Success criteria

- `exec_true` < 74µs (within 1.1x of Linux's 67µs)
- `shell_noop` < 70µs (within 1.1x of Linux's 64µs)
