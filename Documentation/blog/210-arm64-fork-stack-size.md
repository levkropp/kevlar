## Blog 210: ARM64 fork_exit was 3 MiB/fork, not a CoW problem

**Date:** 2026-04-23

Blog 209 ended with Kevlar ARM64 running under HVF, contract parity
at 159/159, and a chart showing `fork_exit` 10.24× slower than
Linux.  The stated hypothesis: port the real ghost-fork CoW path
(a blog-207 known stub) and the derivative fork-heavy benchmarks
should close the gap.

The real bug turned out to be elsewhere, and a lot simpler.

## What I thought was wrong

On x86_64 Kevlar uses `duplicate_table` (regular CoW fork: bump
refcount, clear WRITABLE in both parent and child PTEs) in
`Process::fork`, and `duplicate_table_ghost` (skip refcount,
CoW-mark only, collect cow_addrs) behind the `GHOST_FORK_ENABLED`
flag.  arm64 had a working `duplicate_table` but
`duplicate_from_ghost` just fell back to plain eager copy — the
blog-207 stub.

I ported the real ghost-fork (244 lines, mostly mechanical — see
commit `05bab33`).  It works: vfork_basic passes, all 159 contract
tests under both TCG and HVF stay green.

But it didn't move `fork_exit` at all.  Re-running the benchmark
with the ghost-fork code exercised: still 167 µs per iteration.

## What was actually wrong

Dropped a read_clock_counter at the start and end of
`Process::fork` on arm64, printed every 50 forks:

```
FORK_TIME: n=1   ticks=2559
FORK_TIME: n=50  ticks=2305
FORK_TIME: n=100 ticks=2342
...
```

~2400 ticks per fork.  At CNTFRQ_EL0 = 24 MHz that's **100 µs**
just for `Process::fork` — 60% of the entire fork_exit cycle.

Where was that going?  `parent.arch.fork()` — which on arm64 looks
like:

```rust
let kernel_stack = alloc_pages_owned(
    KERNEL_STACK_SIZE / PAGE_SIZE, ...
)?;
let interrupt_stack = alloc_pages_owned(
    KERNEL_STACK_SIZE / PAGE_SIZE, ...
)?;
let syscall_stack = alloc_pages_owned(
    KERNEL_STACK_SIZE / PAGE_SIZE, ...
)?;
```

Three stacks.  Each `KERNEL_STACK_SIZE / PAGE_SIZE` pages.  Look
at `KERNEL_STACK_SIZE`:

```rust
pub const KERNEL_STACK_SIZE: usize = PAGE_SIZE * 256;
```

**256 pages = 1 MiB per stack × 3 stacks = 3 MiB of buddy
allocation per fork.**

x86_64 uses 8 pages (32 KiB) for the main kernel stack and 2
pages (8 KiB) for interrupt and syscall stacks — 48 KiB total per
fork.  ARM64 was asking the buddy allocator for **64× more memory
per fork than necessary.**

The comment above the constant said only `// Kernel stack size:
256 pages = 1 MiB.` — no rationale.  The most likely origin is
"debugging a deep stack overflow once, bumped it, never reduced."

## The fix

```diff
-pub const KERNEL_STACK_SIZE: usize = PAGE_SIZE * 256;
+pub const KERNEL_STACK_SIZE: usize = PAGE_SIZE * 8;
+pub const AUX_STACK_PAGES: usize = 2;
+pub const AUX_STACK_SIZE: usize = PAGE_SIZE * AUX_STACK_PAGES;
```

Interrupt and syscall stacks now use AUX_STACK_PAGES (2).
`switch_task`'s `sp_el1 = syscall_stack_top` arithmetic and
`rsp_in_owned_stack`'s range check both pick up the smaller size.

Total per-fork stack memory: **3 MiB → 48 KiB (64× reduction)**.

## Results

Kevlar HVF vs Linux HVF, Apple Silicon, same aarch64-linux-musl
binary, same initramfs:

| benchmark      | before          | after          | Linux    |
|----------------|-----------------|----------------|----------|
| fork_exit      | 167 272 ns 10.24× | 109 524 ns 6.77× | 16 173 ns |
| exec_true      | 154 972 ns  4.95× |  84 575 ns 2.44× | 34 685 ns |
| shell_noop     | 188 586 ns  3.99× | 112 147 ns 2.37× | 47 270 ns |

fork itself: 100 µs → 23 µs (4.4× faster on the fork syscall alone).
The derivative benchmarks (`exec_true`, `shell_noop`) halved because
the fork inside them got cheaper.  Every contract test still passes.

## Lessons

Three:

1. **Profile before porting.**  I spent an hour porting ghost-fork
   assuming it was the bottleneck because the stub was documented
   as one.  A `read_clock_counter` around `Process::fork` would
   have told me in 5 minutes that the stub was irrelevant — the
   buddy allocator call was the hot path.

2. **Unjustified constants are suspect.**  A `PAGE_SIZE * 256`
   with no rationale next to an x86 sibling using `PAGE_SIZE * 8`
   is a smell worth investigating even before you measure.  Same
   category as blog 208's buddy bitmap sizing: the comment was
   there, just nobody checked the number.

3. **Ghost-fork was still worth porting.**  The blog-207 stub is
   now real; `vfork_basic` exercises it, the arm64 backend no
   longer lies about CoW semantics, and when `GHOST_FORK_ENABLED`
   flips on (the musl-pthread-init deadlock is a fixable issue,
   per the comment in `process.rs`) arm64 gets the same perf win
   as x86.  Just wasn't the near-term bottleneck.

## What's still slow

After this fix Kevlar is faster than Linux on 32/51 benchmarks, 4
within 10%, 7 within 30%.  The remaining "2-3× slower" outliers
that aren't fork-derived:

- `socketpair`  5.03×
- `pipe`        3.67×
- `read_zero`   3.50×
- `mmap_fault`  3.09×

Each has its own story.  The fork_exit gap (still 6.77×) likely
narrows further with `stack_cache` integration (x86 uses it for
its 2-page aux stacks, reusing freed stacks from a slab-like
cache — arm64 currently allocates fresh from buddy every time).
Each buddy call is ~3 µs; three of them per fork is ~9 µs, nearly
half the remaining 23 µs fork cost.

## Stats

- 1 changed line of real importance (the constant)
- 2 commits (ghost-fork port + stack size reduction)
- 64× less memory per fork
- 33% faster fork_exit, 51% faster exec_true
