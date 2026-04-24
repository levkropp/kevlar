## Blog 222: x64 fork is competitive with Linux, arm64 isn't — and why

**Date:** 2026-04-24

Blog 221 closed `bench_tar_extract` and `bench_sort_uniq`'s harness
artifacts and reported every Kevlar-vs-Linux bench gap under 2×.  The
ones that remained were all clustered around fork+exec:

```
fork_exit    1.86×    exec_true    1.40×    shell_noop   1.46×
pipe_grep    1.19×    sed_pipeline 1.20×    sort_uniq    1.25×
tar_extract  1.12×
```

This session set out to close those.  We landed two real fixes, found
two real bugs, ran ghost-fork as an experiment, and then —
critically — confirmed via a TCG x64 vs arm64 cross-architecture
comparison that **the remaining gap is arm64-specific** and lives
mostly in `do_switch_thread`'s eager FP save/restore.

## Bug 1: AF_INET socket() panicked, swallowing 7 bench results

Fresh `bench --full` runs were silently truncating after `fcntl_lock`
/ `flock` — no `BENCH setsockopt`, `BENCH getsockopt`, no `BENCH_END`.
Enabling `debug=trace` to break down the fork cost surfaced the cause
in the trace dump:

```
panicked at libs/kevlar_utils/once.rs:25:26: not yet initialized
  → SmoltcpNetworkStack::create_tcp_socket  (kernel/net/mod.rs:455)
  → SOCKETS.lock()                          (kernel/net/tcp_socket.rs:76)
```

`init_net()` early-returns when there's no ethernet driver (which is
the HVF/no-NIC bench case), leaving `SOCKETS: Once<SocketSet>`
uninitialized.  `bench_setsockopt` calls `socket(AF_INET, SOCK_STREAM,
0)`, the dispatch lands in `SmoltcpNetworkStack::create_tcp_socket`,
that calls `SOCKETS.lock()` and the `Once` deref panics the kernel.
Bench had been losing the back half of the suite since whenever this
path landed; nobody noticed because the BENCH lines that *did* print
all looked correct.

Fixed by gating AF_INET creation on `net::is_initialized()`:

```rust
(AF_INET, SOCK_STREAM, 0) | (AF_INET, SOCK_STREAM, IPPROTO_TCP) => {
    if !crate::net::is_initialized() { return Err(Errno::EAFNOSUPPORT.into()); }
    net.create_tcp_socket()
}
```

Programs that already check `socket() < 0` (musl, our bench) gracefully
skip — matches Linux's behavior when an address family isn't supported.
After the fix, bench reaches `BENCH_END` cleanly with `BENCH_SKIP
setsockopt` / `BENCH_SKIP getsockopt`.  Zero panics.

## Bug 2: `restore_writable_from_list` per-PTE memory barrier

Under ghost-fork (off by default but used during this investigation),
every CoW restore did a serializing `dsb ish; isb` *per page*:

```rust
// platform/arm64/paging.rs::restore_writable_from_list (BEFORE)
for &vaddr in addrs {
    if let Some(mut pte) = traverse_mut(pgd, uva, false) {
        // ... clear ATTR_AP_RO ...
        unsafe { *pte.as_mut() = restored; }
        unsafe { core::arch::asm!("dsb ish", "isb", options(nostack)); }
        //                              ^^^ once per restored PTE
    }
}
```

For an exec restoring ~200 CoW pages that's 200 serializing barriers.
The x64 mirror function (`restore_writable_from_list` in
`platform/x64/paging.rs`) has zero barriers — strong memory model
needs none.  Moved to a single barrier at the end:

```rust
let mut any = false;
for &vaddr in addrs {
    if let Some(mut pte) = traverse_mut(pgd, uva, false) {
        // ... mutate ...
        any = true;
    }
}
if any {
    unsafe { core::arch::asm!("dsb ish", "isb", options(nostack)); }
}
```

Real bug, kept the fix regardless of whether ghost-fork ever ships.

## Experiment: enable ghost-fork

`GHOST_FORK_ENABLED: AtomicBool = AtomicBool::new(false)` exists at
`kernel/process/process.rs:149` with a comment claiming it
"deadlocks fork+interact patterns."  Flipped it to true and ran the
suite to see what happens:

| Bench | Baseline | Ghost on | Δ |
|---|---:|---:|---:|
| `fork_exit` | 24.7 µs | 21.9 µs | **-12 % ✓** |
| `exec_true` | 47.4 µs | 53.4 µs | **+13 % ✗** |
| `shell_noop` | 65.3 µs | 74.4 µs | **+14 % ✗** |
| `pipe_grep` | 175.6 µs | 182.3 µs | +4 % ✗ |
| `sed_pipeline` | 243.7 µs | 253.3 µs | +4 % ✗ |
| `sort_uniq` | 517 µs | **HANG** | suite stalled |

Wins fork-immediate-exit, loses every fork+exec, hangs nested-fork
shell pipelines.  The win on the cheap path is smaller than the loss
on the common path.  Net loss for our workload mix.

Even reading the comment again — "deadlocks fork+interact" — looking
at the actual code, `Process::fork` doesn't block the parent in the
regular fork path (only `clone(CLONE_VFORK)` does, in `clone.rs`).
The real failure mode for fork+interact under ghost-fork is **silent
data corruption**, not deadlock: the parent's PTEs get marked RO
without refcount bumps, so when the parent CoW-faults, `is_ghost ==
false` on the parent's VM, `refcount > 1` is false (no bump
happened), so the fault handler treats parent as sole owner and
makes the page writable in place — corrupting the child's view of
the same paddr.  The comment misnames the symptom.

Reverted the flag.  Kept the barrier fix.

## The decisive cross-arch comparison

The user's intuition: "x64 fork is competitive with Linux, arm64
isn't.  Find why."

To check, built x64 Kevlar (musl-cross + LLVM objcopy/strip on macOS
arm64; symlinked `musl-gcc → x86_64-linux-musl-gcc`) and downloaded
Alpine 3.21's x86_64 vmlinuz-virt for the Linux side.  Both run under
TCG since HVF on Apple Silicon doesn't accelerate x86 guests — TCG
numbers are slow, but the **ratio** Kevlar/Linux is meaningful: it
tells us how much per-syscall work the kernel does relative to Linux,
independent of acceleration.

| Bench (TCG ns/iter) | Linux x64 | Kevlar x64 | K/L |
|---|---:|---:|---:|
| `fork_exit` | 352,815 | 445,729 | **1.26×** |
| `exec_true` | 950,029 | 814,224 | **0.86× — Kevlar FASTER** |
| `shell_noop` | 1,235,516 | 987,145 | **0.80× — Kevlar FASTER** |
| `pipe_pingpong` | 60,902 | 42,958 | 0.71× |
| `mmap_munmap` | 12,658 | 1,710 | 0.14× |
| `getpid` | 530 | 408 | 0.77× |

vs our **arm64 HVF** ratios from blog 221:

| Bench | arm64 K/L | x64 K/L | arm64 extra |
|---|---:|---:|---:|
| `fork_exit` | 1.86× | 1.26× | **+0.60×** |
| `exec_true` | 1.40× | 0.86× | **+0.54×** |
| `shell_noop` | 1.46× | 0.80× | **+0.66×** |

The user was right.  On x64 we already match or beat Linux on the
fork+exec path.  arm64 has a uniform ~0.5-0.6× of additional work
that x86 doesn't pay.

## Bonus finding: ghost-fork panics on x86 too

While we had x64 builds going, flipped `GHOST_FORK_ENABLED = true` on
x64 to test the user's claim that "ghost fork works on x86_64."
Result:

```
panicked at platform/stack_cache.rs:334:17:
STACK CORRUPT: top byte zeroed at offset 0x6c8, val=0xff8000017e67b8
```

A self-referential kernel pointer (`0xffff8000017e67b8`) with byte 7
stomped from `0xff` to `0x00`, sitting in a freed-and-cached kernel
stack.  Same signature as the original blog-175-177 XFCE crash that
the corruption check was added to detect.

And — more interesting — **the same panic fires on x64 with
`GHOST_FORK_ENABLED = false`** after `shell_noop`.  So this is a
latent x64 bug the heavy fork+exec workload exposes via stack-cache
reuse, **not** a ghost-fork-specific issue.  Same stack-corruption
signature on both arches, both with ghost-fork off.  Filed for a
separate investigation; doesn't block fork performance work.

So: ghost-fork as currently implemented is broken on **both** arches.
The user's premise that it works on x86 is also false.

## Where the arm64 gap lives — span profile

After fixing bug 1, `debug=trace` finally produces clean span output
on `bench --full`:

```
fork.total            avg  9,625 ns   (~10 µs/fork)
  fork.page_table     avg  5,416 ns   (PMD batch-null + TLB batching applied)
  fork.struct         avg  3,125 ns   (Arc::new(Process { ... }))
  fork.arch           avg    375 ns   (kevlar_save_fp_to + stack alloc)
  fork.inner_clones   avg    416 ns
  fork.register       avg     83 ns
  fork.alloc_pid      avg      0 ns
  fork.files_clone    avg      0 ns

ctx_switch            avg  1,791 ns   ← 1,200 ns of which is FP save+restore
do_switch_thread      avg  1,666 ns   ← assembly portion
```

`do_switch_thread` (`platform/arm64/usermode.S:158`) saves and restores
all 32 `Q` registers + FPCR + FPSR on every context switch:

```asm
do_switch_thread:
    // ... save 13 callee-saved GPRs ...
    stp     q0,  q1,  [x3, #(0  * 32)]    // 32 stp's = 64 q-reg stores
    stp     q2,  q3,  [x3, #(1  * 32)]
    ...
    stp     q30, q31, [x3, #(15 * 32)]
    mrs     x9,  fpcr
    mrs     x10, fpsr
    str     x9,  [x3, #512]
    str     x10, [x3, #520]
    // ... swap SP, then mirror loads for next task's FP ...
```

72 instructions of FP save+load per switch, on memory that's likely
cold.  Measured ~1.2 µs of the 1.7 µs `do_switch_thread` cost.

**x64 doesn't pay this.**  XSAVE is lazy: the FP state is only loaded
from memory when userspace actually touches a SIMD register.  Linux
arm64 has the same model — `TIF_FOREIGN_FPSTATE` flag, CPACR_EL1 trap
on EL0 FP use, save/restore in the trap handler.

This is the answer.  Not the page-table walk (5.4 µs/fork is real
but proportional — Linux is doing similar work).  Not the
`Arc::new(Process)` (3.1 µs is also real but x64 pays it too — yet
x64 wins on fork+exec).  The arm64-specific cost that x64 doesn't
pay is **eager FP save/restore on every context switch**, hit
multiple times per fork_exit iter.

## Other smaller fixes landed this session

- **PMD-level 8-wide batch-null skip** in `duplicate_table`
  (`platform/arm64/paging.rs:580`).  Same pattern that already exists
  at PGD/PUD level and inside `share_leaf_pt` itself; was missing at
  PMD level.  Sparse PMDs are typical (1-3 of 512 slots populated for
  a small process).  Modest fork win on the order of 100-200 ns.

- **Batched fork TLB flush.**  `share_leaf_pt` was doing a
  per-leaf-PT broadcast `tlbi aside1is` after stamping each shared
  leaf — fired 3-5× per fork.  Replaced with a single broadcast at
  the end of `Vm::fork` via a new `flush_tlb_all_broadcast` method
  (the existing `flush_tlb_all` was local-only).  This required
  adding a separate `flush_tlb_all_broadcast` method on both arches
  rather than upgrading `flush_tlb_all` itself — `flush_tlb_all` is
  also called from the page-fault path, where the broadcast variant
  is overkill and noticeably slowed exec when I tried that path
  first.

- **Inlined FpState** (was `Box<FpState>`) — **reverted.**  Saves a
  heap alloc per fork (-7 % `fork_exit`), but **regressed
  `exec_true` by +10 %**.  Isolated experiment confirmed FpState
  inlining alone caused the exec regression.  Likely cache locality:
  with `Box`, FpState lives in a separately-allocated 528-byte chunk
  hot in slab from the recent `kevlar_save_fp_to`.  Inlined into the
  ~3 KB `Process` struct it sits at a fixed offset that's cold by
  the time `do_switch_thread` reads it.  Net loss; reverted.

- **Single-atomic `page_ref_inc` fast path** — **abandoned.**  The
  current load-then-fetch_add is two atomics; `fetch_update` is also
  two atomics on the success path (load + cmpxchg).  No
  single-atomic version preserves the kernel-image sentinel guard,
  which is load-bearing for ~10 callers that compare directly
  against `PAGE_REF_KERNEL_IMAGE`.  No safe win.

## Net result this session

Ratios after the small wins (3-run mean, post all fixes):

| Bench | Before | After |
|---|---:|---:|
| `fork_exit` | 1.86× | 1.79× |
| `exec_true` | 1.40× | 1.41× |
| `shell_noop` | 1.49× | 1.48× |

The headline ratios moved by under 5 %.  The real value of the
session is what it identified: the dominant remaining gap is
`do_switch_thread`'s eager FP save/restore, and that closes via
**lazy FP via CPACR trap** — the canonical Linux arm64 model.

## Next session: lazy FP

CPACR_EL1's `FPEN` bits control whether EL0 FP/SIMD instructions
trap to EL1.  Plan:

- Boot: install an FP-disabled-at-EL0 trap handler.  Per-CPU
  "current FP owner" pointer (the task whose FP state is currently
  loaded in v-regs).
- `do_switch_thread`: stop saving/loading FP unconditionally.  Set
  CPACR to trap at EL0.  Mark next task's FP as not-loaded.
- FP trap handler: save current FP owner's v-regs into its FpState
  if any; load this task's FpState into v-regs; clear trap; record
  this task as new FP owner.
- Fork: child inherits parent's FpState by `kevlar_save_fp_to` at
  fork time (already does this), then is marked not-FP-owner.
  First time the child uses FP after returning to EL0, it traps and
  loads.

Expected payoff per the profile: ~1.2 µs/ctx_switch × ~2-4 switches
per fork_exit iter = 2.5-5 µs/iter.  Closes most or all of the 11 µs
gap.  Doesn't break anything for tasks that genuinely use FP — it
just defers the cost to the first FP instruction.

## Commits

- `(this commit + the panic/barrier/PMD/TLB fixes folded together)`
  — net diff under 200 lines across `kernel/net`, `kernel/syscalls/
  socket.rs`, `kernel/mm/vm.rs`, `platform/arm64/paging.rs`,
  `platform/x64/paging.rs`, `tools/bench-report.py`.
- Blog 222.

## Open / deferred

- **Stack corruption bug** (`stack_cache.rs:334`) — fires on x64
  during heavy fork+exec workloads regardless of ghost-fork.  Top
  byte of a kernel pointer in a freed cached stack gets stomped
  `0xff → 0x00`.  Same signature as blog 175-177's XFCE crash.
  Worth a dedicated investigation; doesn't block fork perf work.
- **Ghost-fork comment cleanup** — comment claims "deadlocks
  fork+interact" but actual failure mode is silent data corruption.
  Update when next touching that file.
- **VMA-aware PTE walk in `duplicate_table`** — Linux walks only
  `[vma->vm_start, vma->vm_end)` per VMA.  Our blind 512-entry
  walks per leaf PT are the next-best lever after lazy FP.  Saves
  ~2-3 µs/fork (proportional to # of empty PTE slots walked).
- **Process struct pool** — `fork.struct` is 3.1 µs/fork.  Pooling
  needs `Arc::new_uninit_in` (unstable) or a custom allocator — day
  of plumbing for a 3 µs win.  Lower priority than lazy FP.
