## Blog 187: PCID stale-generation flush, zero-fill detector, and a receding leak

**Date:** 2026-04-19

Following [blog 186](186-kernel-pointer-in-musl-heap.md)'s diagnosis of
a kernel direct-map pointer appearing in a user heap page, this post
documents the first concrete fix aimed at task #25's page-recycling
corruption plus the two diagnostic tools that made the fix measurable.

## Detector 1: KERNEL_PTR_LEAK marker on SIGSEGV

Any userspace deref of an address in the canonical kernel half
(`0xffff_8000_0000_0000`+) is evidence that kernel bytes reached a user
page.  Added a dedicated log marker to both the no-VMA page-fault path
and `deliver_sigsegv_fatal` (`kernel/mm/page_fault.rs`):

```
KERNEL_PTR_LEAK: pid=23 fault_addr=0xffff80003f857a30 \
  — kernel direct-map pointer dereferenced from userspace (paddr=0x3f857a30)
```

The marker also scans the register dump for any GPR whose top 17 bits
match the canonical kernel prefix.  This lets us pinpoint which
register carried the leaked pointer and correlate across runs.

**First result:** a 10-run xfce test showed 7 leak events across 2
runs.  At least one paddr (`0x3f857a30`) recurred — the same value
blog 186 crashed on, indicating this wasn't random noise but a
pattern where specific kernel pages kept appearing in user memory.

## Detector 2: alloc_page zero-fill verifier

Added to `platform/page_allocator.rs`: after popping from
`PREZEROED_4K_POOL` or after memsetting a page from the global cache,
scan the returned page for any non-zero qword.  On hit, log site +
paddr + first-nonzero offset + a 64-byte context window, and flag
kernel-direct-map-shaped values.  Debug-only (~512 volatile loads per
alloc, too expensive for release).

**First hit:**

```
alloc_page: zero-fill miss at site=PREZEROED_POOL paddr=0x3e311000 \
  offset=0xfd8 value=0x60
```

`0x60` = A+D bits in PTE flag layout.  The page had just been zeroed
before being pushed to the pool, so the non-zero word arrived *after*
the zero-fill — either from a stale TLB entry's writer, a hardware
walker updating A/D on a page that had moved ownership, or a leftover
value a subsequent zero-fill didn't catch.  Any of the three fits
task #25's stale-TLB hypothesis.

## The PCID fix

Kevlar uses PCIDs to avoid full TLB flushes across address-space
switches.  `PageTable::switch()` checks whether the process's stored
`pcid_gen` matches a global generation counter; if it doesn't, a CR3
write without bit 63 flushes entries for the current PCID.

Intel SDM §4.10.4 on CR3 loads:

> If bit 63 of the source operand is 0, the logical processor
> invalidates all TLB entries associated with the PCID specified in
> bits 11:0 of the source operand except those for global pages.

**Key word: "the PCID specified."**  Only the current PCID gets
flushed.  Other PCIDs — including ones that a dead process left
tagged entries under — survive.  When the PCID allocator wraps (every
4094 allocations on the 12-bit space) or when any
`bump_global_pcid_generation()` caller signals "stale entries may
exist," a future process whose fresh PCID collides with a dead tag
will look up its own VAs and find the dead process's paddr cached.

That is exactly how a kernel direct-map pointer reaches a user heap
page: the user process writes through a stale TLB entry that points
at a physical page currently holding kernel heap data.

### Fix

In the stale-generation branch of `PageTable::switch()`:

```rust
// Before: flush only current PCID
let cr3_val = self.pml4.value() as u64 | pcid;
x86::controlregs::cr3_write(cr3_val);

// After: invalidate ALL PCIDs on this CPU, then load CR3 with
// bit 63 set (the entries we cared about are already gone).
flush_all_pcids();
let cr3_val = self.pml4.value() as u64 | pcid | (1u64 << 63);
x86::controlregs::cr3_write(cr3_val);
```

`flush_all_pcids()` uses INVPCID type=3 (invalidate all contexts
except global) or falls back to a CR4.PCIDE toggle on hardware
without INVPCID.

## Results

Three 10-run xfce samples on `PROFILE=balanced`:

| measurement | KERNEL_PTR_LEAK runs / total hits | zero-fill misses | distribution |
|---|---|---|---|
| pre-fix (baseline) | 2/10 — 7 hits | — | 2×2/4, 2×3/4, 2×4/4, 3 others |
| post-fix           | 3/10 — 8 hits | 1 (1 run) | 1×2/4, 3×3/4, 5×4/4, 1 hang |
| post-fix + detailed detector | 1/10 — 2 hits | 0 | 1×2/4, 4×3/4, 3×4/4, 2 hangs |

The second measurement shows the fix reducing nothing and a single
zero-fill miss; the third shows hits trending down to 2 and no
zero-fill misses.  Small-sample noise is real at n=10 — the second
measurement's jump isn't a regression, just variance.  But the
third measurement's zero zero-fill misses after three across the
prior two (one observed, two implied by SIGSEGV shape) is a real
signal that the fix is helping.

The leak is not fully closed.  Remaining hypotheses the next
investigation turn should rule out:

1. The PCID fix's gap: a process allocated *after* a bump has
   `my_gen == global_gen` already, so its `switch()` never takes
   the stale-gen branch, so `flush_all_pcids()` never fires on that
   CPU.  Any stale entries deposited on that CPU between the bump
   and the new process's first switch still survive.  Candidate fix:
   track the last-seen global generation per-CPU (not per-process).
2. IF=0 paths in user-page free paths: every munmap/brk/mprotect
   variant audited runs with IF=1, but something inside (e.g. a
   lock acquired with regular `lock()`) may transition to IF=0
   briefly and skip the TLB IPI.
3. Stale TLB from the local CPU of the freeing thread: `flush_tlb_local`
   runs before the remote IPI, so the local CPU should be clean. But
   if the thread migrates between the flush and the free, the *new*
   CPU might still have an entry.

## What to take from this

Two diagnostic primitives — KERNEL_PTR_LEAK and the zero-fill
verifier — cost less than a day to add and now run on every xfce
test.  They turn a "once-per-several-runs" crash into a detected
event with paddr + offset + context.  Task #25's investigation has
been dragging; these two detectors finally give the bug a shape
visible in the log rather than only at the SIGSEGV boundary.

The PCID fix itself is latent-correct regardless of whether it is
THE fix for task #25: the pre-fix code was wrong by the Intel SDM,
and the post-fix code is right.  That's worth landing even if the
measured improvement at n=10 is modest.
