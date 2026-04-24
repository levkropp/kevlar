## Blog 220: arm64 exec + fork, continuing on

**Date:** 2026-04-24

Blog 219 closed three fixes on the exec path (2A / 2B / 2C):
`apply_prefault_template` batching, a diagnostic double-read gated
behind `profile-fortress`, and `prefault_small_anonymous` +
init-stack batching.  Headline was `exec_true` 53 → 48 µs
(-10 %) and `fork_exit` 29 → 25 µs (-13 %) as incidental benefit.

This post picks up where 219 left off: instrumented `fork.page_table`
to find where its 4.5 µs/fork actually lives, found the one flush
that was wasting the most cycles, and narrowed it.

## Fork breakdown

With `KEVLAR_DEBUG=trace` and span IDs added around
`PageTable::duplicate_from`, `flush_tlb_all`, and the `vm_areas.clone`
step, one fork produces:

```
fork.total              calls=200 avg=7333 ns
  fork.page_table       calls=200 avg=4541 ns
    fork.pt_duplicate   calls=200 avg=4291 ns   (94%)
    fork.pt_flush       calls=200 avg= 166 ns   (4%)
    fork.vma_clone      calls=200 avg=   0 ns
  fork.arch             calls=200 avg= 375 ns
  fork.struct           calls=200 avg=1625 ns
  fork.inner_clones     calls=200 avg= 416 ns
```

Every bit of the 4.5 µs attributed to `fork.page_table` is in
`PageTable::duplicate_from`.  `flush_tlb_all` (the outer
`tlbi aside1` after CoW marks) is 166 ns — the ASID work from blog
218 already made that cheap.  The `vm_areas` vec-clone is free.

## What `duplicate_from` does

Recursive walk:
- **PGD (level 4)**: 1 table, 1-2 non-null entries (user-range PUD).
  Alloc + `memcpy 4 KB` + recurse per non-null.
- **PUD (level 3)**: same, 1-2 non-null.
- **PMD (level 2)**: for each non-null PMD entry, call
  `share_leaf_pt()` — sets `ATTR_AP_RO` on writable PTEs in-place,
  bumps per-data-page refcounts, bumps pt-refcount, publishes the
  `PTE_SHARED_PT` bit on parent + child's PMD entry.
- **Leaf PT (level 1)**: *not* allocated — shared via refcount,
  that's the blog 212 lazy leaf-PT-CoW design.

`share_leaf_pt` is called once per PMD entry that points at a
populated leaf PT — for busybox, 3-5 times per fork.

## The overbroad broadcast

Inside `share_leaf_pt`, after the in-place RO-stamp pass, there's a
TLB fence:

```rust
// Flush all TLB entries in the inner-shareable domain.
tlb_flush_all_broadcast();  // → tlbi vmalle1is
```

That flushes **every entry in every CPU's TLB** for the whole
inner-shareable domain.  It's there for correctness on SMP-threaded
forks: another thread on another CPU with stale writable TLB entries
for the CoW'd range could otherwise write through to the (now
shared) data page and corrupt the child's view.

The flush is called 3-5 times per fork — once per leaf PT shared.
But each one was nuking every other process's TLB entries too.

## Narrowing to the parent's ASID

With ASID-tagged TLBs from blog 218, we can use
`tlbi aside1is, xN` — broadcast but scoped to parent's ASID only.
Same correctness (the stale writable entries that need invalidation
are all tagged with parent's ASID), spares every other process on
the host.

Threaded the parent's ASID through `duplicate_table` → `share_leaf_pt`
and added `tlb_flush_asid_broadcast(asid)`:

```rust
#[inline(always)]
fn tlb_flush_asid_broadcast(asid: u64) {
    let operand = asid << 48;
    unsafe {
        core::arch::asm!(
            "dsb ishst",
            "tlbi aside1is, {x}",
            "dsb ish",
            "isb",
            x = in(reg) operand,
            options(nostack),
        );
    }
}
```

On our single-CPU HVF bench this is within noise (no other
processes to spare).  The real payoff is on multi-tenant /
container-style workloads where a fork in one process shouldn't
disturb another's TLB.  Ship it regardless — it's a correctness
narrowing that matches how Linux models TLB fences.

Contracts 159 / 159, threads-smp 14 / 14.

## Where we are vs Linux

Updated Kevlar-vs-Linux ratios, both `bench --full`:

| Bench | Linux | Kevlar (pre) | Kevlar (now) | Pre-ratio | Now-ratio |
|---|---:|---:|---:|---:|---:|
| `tar_extract` | 12.6 ms | 468 ms | 443 ms | 37× | 35× |
| `sort_uniq` | 13.1 ms | 471 ms | 424 ms | 36× | 32× |
| `sed_pipeline` | 14.9 ms | 291 ms | 249 ms | 19× | 17× |
| `pipe_grep` | 14.1 ms | 190 ms | 183 ms | 14× | 13× |
| `shell_noop` | 13.2 ms | 70 ms | 65 ms | 5.3× | 4.9× |
| `exec_true` | 13.0 ms | 53 ms | 49 ms | 4.1× | **3.8×** |
| `fork_exit` | 14.9 µs | 29 µs | 25 µs | 1.9× | **1.7×** |

`fork_exit` closed 11 % of the gap, `exec_true` 10 %.  Pipeline
benches compressed proportionally to the exec improvement beneath
them — `sort_uniq` dropped from 36× to 32×, which is a 47 ms/iter
reduction even though the per-exec cost only moved a couple of
microseconds.

## What's still expensive

From the fork trace, biggest remaining components per fork_exit
iter:

- `fork.pt_duplicate` 4.3 µs — the recursive PGD/PUD/PMD walk +
  `share_leaf_pt` per leaf PT.  Most of the cost is the memcpy of
  intermediate-level tables and the refcount-bump loop inside
  `share_leaf_pt`.  Compressible but not obviously.
- `fork.struct` 1.6 µs — `Arc::new(Process { ... })`.  ~2.5 KB
  heap alloc + zero-init.  A per-CPU Process-pool would compress
  this to sub-µs, but needs careful lifetime plumbing since Arc
  allocates the control block separately.
- `ctx_switch` / `sys_wait4` — dominates the iter when measured
  wall-clock but it's mostly "other process running," not kernel
  work.

From the exec trace, biggest remaining:

- `exec.prefault` 7-8 µs — template-cache-warm path applying
  ~260 entries through batched `tlbi`.  After 2A this is as tight
  as the primitive allows; further wins would need a 64-wide batch
  primitive (u64 return) or huge-page template entries instead of
  4 KB runs.
- `exec.hdr_read` 1.4 µs — reads the 4 KB ELF header from file.
  File cache hit; still 1.4 µs including the file-object vtable
  and refcount work.

## Commits

- `7505f6a` — `arm64 paging: 8-wide batch-null skip in
  intermediate-level duplicate_table`.  PGD / PUD walks now skip
  runs of 8 null entries in one OR+load.  Neutral on single-CPU
  bench but shaves some iterations when the upper levels are
  sparse.
- `4a81c95` — `arm64 paging: ASID-scoped broadcast flush in
  share_leaf_pt`.  `tlbi aside1is` instead of `tlbi vmalle1is`
  when parent has a non-zero ASID.

## Next

The per-process arithmetic is getting thin.  The remaining 35 µs
Kevlar-over-Linux on `exec_true` is spread across many micro-phases
of 100-500 ns each; every 500 ns fix takes a session to find, land,
and verify.

Where the scale-free wins are:

1. **`tar_extract` 35×** is not a fork/exec problem.  tar runs as
   a single process doing many `open` + `read` + `write` +
   `mkdir`.  Directory-path-lookup cost or file-object allocation
   is the suspect.  A direct profile on `tar_extract` would tell
   us which syscall is the hot spot.
2. **`sort_uniq` 32×** runs `sort | uniq` — two long-running
   children with pipe IPC.  Exec-path batching helped; the
   residual is likely pipe throughput or userspace sort's data
   model.
3. **`Arc<Process>` pool** — the only clear single-µs-win left
   on the fork path.

Probably worth profiling `tar_extract` next to confirm or refute
that it's a filesystem-path problem.  The user-facing XFCE
workload does a lot of file opens for icons / config / theme data,
so anything that speeds up `open` path lookup pays dividends
beyond the bench.