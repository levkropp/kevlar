## Blog 203: PCID generation tracking moves from per-process to per-CPU

**Date:** 2026-04-21

Task #25 — kernel pointers leaking into user pages — has had the same
shape since [blog 186][186]: some user VA resolves to a physical page
that holds kernel data, so userspace reads a kernel direct-map pointer
(`0xffff_8000_xxxxxxxx`) out of its own heap and page-faults on the
next dereference.  [Blog 187][187] landed the first PCID fix (flushing
*all* PCIDs instead of just the current one on a stale-generation
switch), documented three remaining hypotheses, and left the leak
partially open.

This post implements hypothesis #1 and measures the result.

## The hole in per-process tracking

Each `PageTable` carried a packed `pcid_gen = (gen << 12) | pcid`.
`PageTable::switch()` compared it to a global generation; mismatch
triggered `flush_all_pcids()` on the current CPU.  The per-process
field was also the return of `alloc_pcid()` — so a freshly allocated
process starts with `my_gen == global_gen` and its first `switch()`
takes the fast path.

`alloc_pcid()` wraps at 4095 allocations:

```rust
if next >= 4095 {
    let new_state = (generation + 0x1000) | 1;   // bump gen, reset PCID→1
    if PCID_STATE.compare_exchange_weak(...).is_ok() {
        return new_state;                        // new_gen | PCID=1
    }
}
```

The PCID it returns is **recycled** — some earlier process that owned
PCID=1 just exited.  That earlier process's switch()es on every CPU
filled TLB entries tagged `(PCID=1, old_gen)`.  They're still in the
TLB because nobody has called `flush_all_pcids()` on those CPUs since
the earlier process exited.

Under the Intel SDM, PCID tagging is just bits 11:0 — the hardware
doesn't know about our `gen` half.  It sees PCID=1 entries in the TLB
and matches them against any CR3 load using PCID=1.

The fresh process now walks with CR3 | PCID=1 | bit-63 (no-invalidate).
Its `switch()` saw `my_gen == global_gen` and skipped the flush.  So
on CPUs the new process touches, the hardware reuses the recycled-tag
entries from the prior PCID=1 owner.  Those entries map the prior
owner's user VAs to the prior owner's physical pages — possibly
freed, possibly reassigned to kernel heap, possibly holding kernel
direct-map pointers.

That's how `0xffff800002d074e8` ends up in a user heap page.

## The fix: per-CPU last-seen generation

Move the generation check out of `PageTable` and into a `MAX_CPUS`-
sized array of atomics:

```rust
static CPU_LAST_SEEN_GEN: [AtomicU64; super::smp::MAX_CPUS] = ...;

pub fn switch(&self) {
    let pcid = self.pcid_gen.load(Relaxed) & 0xFFF;
    let global_gen = PCID_STATE.load(Relaxed) & !0xFFF;
    let cpu = cpu_id() as usize;
    let cpu_gen = CPU_LAST_SEEN_GEN[cpu].load(Relaxed);

    if cpu_gen == global_gen {
        // fast path — this CPU has flushed for the current generation
        cr3_write(pml4 | pcid | (1 << 63));
    } else {
        flush_all_pcids();
        CPU_LAST_SEEN_GEN[cpu].store(global_gen, Relaxed);
        cr3_write(pml4 | pcid | (1 << 63));
    }
}
```

The invariant: after any `bump_global_pcid_generation()` call, the
next `switch()` on every CPU flushes — independent of which process
is switching.  A freshly-allocated process whose `my_gen == global_gen`
no longer gets a free pass: if *this CPU* hasn't flushed for the
current generation yet, it flushes now.

Preemption is disabled by the scheduler around `switch()`, so
`cpu_id()` is stable and only one writer (this CPU) ever stores to
`CPU_LAST_SEEN_GEN[cpu]`.  `INVPCID` is a serializing instruction, so
the store-after-flush happens after any in-flight speculative walks.

## Results

The [task #25 leak detector][187] has been running on every
`test-xfce` run for weeks, counting user dereferences of
kernel-direct-map pointers.  Pre-fix numbers from 24 logged runs in
`/tmp/kevlar-xfce-*.log`:

| paddr              | hits | ip range  | process         |
|--------------------|------|-----------|-----------------|
| 0x2d074d8 / 0x2d074e8 | 2+2  | ld-musl  | xfce4-session (x2) |
| 0x22c9f7 / 0x22ca07 | 1+1  | ld-musl   | xfce4-session |
| 0x2d0e9b8, 0x2aeadff0, 0x361b40b0, 0x384dc076, 0x3d1e2880, 0xa16de7e18 | 1 each | varies | session / panel / Xorg |

Same paddrs recurring across runs and processes — the signature of a
systematic recycling hole rather than single-shot corruption.

Post-fix, running `test-xfce PROFILE=balanced` on a fresh kernel:

- **Run 1**: 5/5 TEST_PASS (mount_rootfs, xfwm4, panel, session,
  `xfce_pixels_visible`), **0 KERNEL_PTR_LEAK**.

This is the first full clean pass of the xfce pixel-visibility test
after the PCID fix landed.  It doesn't close task #25 — blog 187's
hypotheses #2 (IF=0 free paths) and #3 (thread-migration-between-
flush-and-free) are still live, and at least one run during
validation showed a residual leak in `xfce4-power-manager`.  But the
per-CPU flip closes the largest identified hole, and the cost is a
single extra atomic load per switch — no measurable overhead on the
14/14 `test-threads-smp` run.

## What remains

The residual leak in run 2 (`paddr=0x29363fc8`, `xfce4-power-manager`)
has the same shape but a different paddr.  Remaining hypotheses:

1. **A free path that doesn't bump the generation.**  Every
   TLB-flushing path calls either `tlb_remote_flush_all_pcids()` (IPI
   path) or `bump_global_pcid_generation()` (IF=0 fallback).  If any
   user-page free path skips both — grep for `free_pages` without a
   preceding flush — a stale TLB entry on a remote CPU outlives its
   paddr.  This is an audit job.

2. **Kernel allocator reusing the same page hot-loop for kernel and
   user workloads.**  If a `USER | DIRTY_OK` allocation slot is also
   the preferred kernel-heap slot, a page just freed from a user
   mapping gets written by kmalloc the next moment — but stale TLB
   on a remote CPU still translates a different user VA to that
   same paddr.  Userspace reads kernel data.  This needs a slab-
   level audit: check whether kernel-heap churn uses pages that were
   ever returned to the common free list from user VMAs.

3. **PT-page walker race from fork CoW** (blog 194).  Not this fix's
   target but worth keeping in mind — it manifests as PT-cookie
   corruption, not register-value corruption, so it's a different
   panic class.

The per-CPU generation fix is a strict improvement regardless: it's
latent-correct per Intel SDM §4.10.4 and closes a category of leak
that the per-process design could never catch.

[186]: 186-kernel-pointer-in-musl-heap.md
[187]: 187-pcid-tlb-leak-fix.md
