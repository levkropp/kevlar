## Blog 212: lazy leaf-PT-page CoW on arm64 (iteration 1)

**Date:** 2026-04-23

Blog 211 pointed the arc at lazy PT-page CoW.  This post is iteration 1 of
that plan: share **leaf** PT pages (level-1, containing the final PTEs)
between parent and child at fork time instead of eagerly duplicating
them.  Intermediate levels (PGD/PUD/PMD) stay eager for now — that's
iteration 2.

Contract suite stayed at 159/159 under HVF.  fork_exit perf moved by
about a microsecond — inside the noise.  That's not the takeaway.  The
takeaway is the infrastructure: `PTE_SHARED_PT`, `pt_refcount`, and
`traverse_mut` are the primitives the rest of the lazy-CoW arc builds
on, and they work.

One bug ate most of the session.  Worth documenting in detail because
it's the kind that reproduces *only* under the new sharing, doesn't
fire at all during the initial fork, and when it does fire it
corrupts userspace in a way that looks like a compiler miscompile.

## The design (from the plan file)

Lazy leaf-PT sharing uses three pieces:

1. **`PTE_SHARED_PT` = bit 56** on a level-2 (PMD) descriptor.  Bit 56
   is in ARMv8's software-use range (bits 55–58 on table descriptors
   are IGNORED by the page-table walker).  When set, it means "the
   PMD entry's target leaf PT is shared with one or more other Vms."
2. **`pt_refcount`** (new: `platform/pt_refcount.rs`), a
   per-PFN `AtomicU16` array, separate from `page_refcount` which
   tracks data pages.  A PT page and a data page never coexist as
   both at once, but their refcount lifecycles differ, so two
   independent arrays.  1 M pages × 2 bytes = 2 MiB static.
3. **`traverse_mut`**, a write-intent version of `traverse`.  When
   descending into a leaf PT whose PMD descriptor has the SHARED
   bit set, it copies the PT up before returning a pointer into it.
   Real unshare when `pt_refcount > 1`; sole-owner fast path (just
   clear the stale SHARED bit) when `pt_refcount == 1`.

Fork's `duplicate_table` at level 2 calls `share_leaf_pt` on each PMD
entry: stamps `ATTR_AP_RO` on every user+writable PTE in the shared
PT, `dsb ish`, `pt_ref_inc`, then `| PTE_SHARED_PT` on both parent's
and child's PMD entries.  Teardown at level 2 with SHARED calls
`teardown_leaf_pt_shared` to dec data-page refcounts for every
owner's teardown (not just the last — the arithmetic only balances
if each fork's +1 gets paired with each teardown's −1), then
`pt_ref_dec` and frees the PT if it was the last owner.

Read-only helpers stay on `traverse`.  Eight mutation-site helpers
(`map_page`, `try_map_user_page_with_prot`,
`batch_try_map_user_pages_with_prot`, `unmap_user_page`,
`update_page_flags`, and the ghost-fork restore path) now route
through `traverse_mut` / `traverse_to_pt_mut`.  Unshare happens
transparently during the walk.

The agent review from the planning pass caught most of the subtle
points up front: RO-stamp-then-publish ordering at fork, eager
`SHARED` clear on the sole-owner fast path so the invariant stays
simple, every teardown decrementing data-page refcounts, ghost-fork
unsharing the parent side before CoW-stamping to avoid leaking
state into the other owner's view, `tlbi vmalle1is` instead of local
flushes.

Land it, compile clean, run `exit_group_basic` under HVF.

## The bug

Parent forks, child calls `exit_group(42)`, parent hangs.  Contract
times out.

Added tracing.  The pattern was striking:

```
SHARE_LEAF_PT n=0 pt_paddr=0x42559000
SHARE_LEAF_PT n=1 pt_paddr=0x42555000
PF n=7  pid=1 vaddr=0x9ffffee20 ip=0x4001d0 w=true  p=true   [parent stack CoW]
UNSHARE n=0 vaddr=0x9ffffe000 pt_paddr=0x42555000
PF n=8  pid=2 vaddr=0x420650   ip=0x403738 w=true  p=true   [child data CoW]
UNSHARE n=1 vaddr=0x420000     pt_paddr=0x42559000
EG pid=2 status=42                                           [child exits]
INST_ABORT n=4 pc=0x420168 lr=0x420168 sp=0x9ffffee10        [parent resumes]
PF n=9  pid=1 vaddr=0x420168   ip=0x420168 w=false p=true if=true
```

Parent resumed from `wait4` with PC = **0x420168**.  That's not code.
`nm` confirmed:

```
0000000000420168 B __libc
```

0x420168 is the *address* of the `__libc` pointer variable in BSS.
Something had loaded 0x420168 into x30 and `ret`'d.  The parent was
looping in a fault at PC=0x420168 forever — the instruction-abort
handler saw it as a permission fault on a present page, called
`update_page_flags` with the VMA's prot (R+W, no X — which is what
the VMA actually is for BSS), and bounced back to user which
faulted again on the same instruction fetch.  The fault handler was
right; the VMA *is* non-executable.  The problem was that parent's
user code got there at all.

### The misdirection

Initial hypothesis: AP_RO stamping corrupted something, or the
`PTE_SHARED_PT` bit was accidentally interpreted by the MMU.  I
bisected `share_leaf_pt` by progressively disabling pieces:

- Disable AP_RO stamping → still fails.
- Disable SHARED-bit publish → still fails.
- Disable `page_ref_inc` → still fails.
- Disable `pt_ref_inc` → still fails.

With everything disabled, `share_leaf_pt` did nothing except let the
bulk-copied PMD entries continue to point at the shared PT.  Which
meant both parent's and child's PMDs pointed at the same PT — with
no CoW mechanism.  The child's writes now go straight through to
pages the parent is also reading.  Expected to fail; not the same
bug.

Re-enabled everything.  Bug reappeared.  The bug is somewhere else.

### The real cause

The giveaway was in the child's unshare trace.  The log said
`UNSHARE n=1 vaddr=0x420000 pt_paddr=0x42559000` with no
`ALLOC_PT` between them.  `unshare_leaf_pt` only calls
`alloc_pt_page` on the real-unshare path (refcount > 1); the
sole-owner fast path just clears the SHARED bit and returns the old
PT pointer.

But the child *should* have been on the real-unshare path.  Parent
and child had both just forked.  `share_leaf_pt` had called
`pt_ref_inc`.  Refcount should be 2.

Unless `pt_ref_inc` ran on a PT whose refcount started at **0**.
Then +1 = 1, and the child sees sole-owner.

```rust
// traverse() — the on-demand allocation path
if table_paddr.value() == 0 {
    if !allocate { return None; }
    let new_table =
        alloc_pages(1, AllocPageFlags::KERNEL).expect("failed to allocate page table");
    ...
}
```

`alloc_pages` alone.  No `pt_ref_init`.  Leaf PTs that were
demand-allocated during initial page faults (before any fork
happened) had `pt_refcount = 0`, not 1.  When fork later called
`share_leaf_pt` on them, `pt_ref_inc` brought them to 1.  Child's
unshare saw refcount=1, took the sole-owner path, returned the old
PT pointer — and then `map_user_page_with_prot` wrote the child's
CoW'd data PTE *into the parent's PT*.

Now the parent's PMD still points at the (unchanged, AP_RO-stamped)
shared PT, except slot 32 has been overwritten by the child's new
mapping.  Parent's view of `0x420000` is now the child's CoW'd
page.  But other slots still reference the original data pages.
What sits at offset 0x168 of the child's CoW'd BSS page is
*whatever the child wrote* into its stack-walking prologue — which,
via a chain of stp/ldp mis-stashes when musl's startup code spilled
`x20` (= 0x420168, from `adrp+add` for `__libc`) onto the stack
frame where x30 also lives, ended up being the address `0x420168`
itself.  `ldp x29, x30, [sp], #16; ret` → PC = 0x420168 → boom.

The fix is one line of meaning, four sites of code:

```diff
-    let new_table = alloc_pages(1, AllocPageFlags::KERNEL).expect(...);
+    let new_table = alloc_pt_page().expect(...);
```

Every PT-page allocation — the four on-demand sites in
`traverse`/`traverse_to_pt` plus the existing `duplicate_table`
sites — goes through `alloc_pt_page`, which `pt_ref_init`s the
fresh PT to 1.  Re-ran `exit_group_basic`: CONTRACT_PASS.  Full
suite: 159/159.

### Lessons

**Invariant:** "refcount > 1 means shared" is correct for the
*lifecycle the refcount tracks*.  But the baseline has to be right.
If a PT is born with refcount 0 because one allocation site
forgot to initialize it, sharing cannot be distinguished from
sole-ownership, and the sole-owner fast path silently miscounts
the first fork.

**Diagnostic sequence that worked**: when bisecting
`share_leaf_pt`'s pieces didn't converge on a single offender, that
was the signal that the bug wasn't *in* `share_leaf_pt` — it was in
a precondition for it.  The absence of an `ALLOC_PT` log entry
where one was expected became the single piece of evidence that
pointed the rest of the way.  I almost didn't log `alloc_pt_page`
because it felt orthogonal.

**Not a review miss.** The plan specified `alloc_pt_page` for
`allocate_pgd`, `duplicate_table`, `duplicate_table_ghost`, and
`traverse_mut`'s unshare path.  The four `alloc_pages` sites in
`traverse`/`traverse_to_pt` are pre-existing code from the eager
world where `pt_refcount` didn't exist.  They were touched only by
the demand-fault path, which ran before any fork.  Anyone reading
just the diff wouldn't spot them.  Good reminder that
"infrastructure that assumes all allocation goes through X" needs
an audit step for "all current allocation sites", not just "where
my new code allocates".

## Why iteration 1 doesn't move fork_exit much

Measured: ~100 µs after, ~101 µs before.  Within benchmark noise.

The blog-211 profile said 80 % of fork's cost is PT duplication.
Of that PT cost, leaf PTs are about 25 % — two PT pages at ~1.2 µs
each.  So iteration 1's maximum achievable win is ~2 µs.

But iteration 1 doesn't *eliminate* the leaf-PT copy; it *defers*
it.  For `fork_exit`, both sides write:

- Parent writes to stack (musl's frame-save after the fork syscall
  returns) → CoW fault → unshare (real path, alloc + memcpy).
- Child writes to data/BSS (writing to TLS for a raw syscall call
  pattern, or similar) → CoW fault → unshare (real path, alloc +
  memcpy).

So both leaf PTs get duplicated anyway, just lazily.  The only
savings is if the cost shifts off-critical-path (into idle
post-fork) or amortizes across multiple subsequent accesses.  In
fork_exit's tight loop, there isn't any such amortization window.

Where iteration 1 *will* win is intermediate PT sharing in
iteration 2.  For a fork_exit child that immediately exec's:

- Child's write pattern before exec = **zero PTEs**.
- Iteration 2 shares PGD/PUD/PMD tables too.
- Child never writes through any shared PT before exec drops
  everything.
- Fork cost collapses to "copy the top-level table, pt_ref_inc the
  children".

That's the ~12 µs win we actually need to close the gap with
Linux's 16 µs fork.

## What's committed

Single commit `ecbd48c`:

- `platform/pt_refcount.rs` new.
- `platform/lib.rs` exports it.
- `platform/arm64/paging.rs` ~440 lines changed:
  - `PTE_SHARED_PT`, `alloc_pt_page`, `tlb_flush_all_broadcast`
  - `unshare_leaf_pt`, `traverse_mut`, `traverse_to_pt_mut`
  - `share_leaf_pt`, `teardown_leaf_pt_shared`
  - `duplicate_table` level-2 branch now shares instead of recursing
  - `teardown_table_dec_only` / `teardown_table_ghost` handle SHARED
  - `duplicate_table_ghost` unshares parent side first
  - `restore_writable_from_list` uses `traverse_mut`
  - Eight modification-site helpers rewired to `traverse_mut`
  - Four on-demand PT alloc sites routed through `alloc_pt_page` —
    the bug fix

## Next

Intermediate PT sharing.  Same `PTE_SHARED_PT` bit, same
`pt_refcount`, applied at the PMD-on-PUD and PUD-on-PGD levels.
The trickier part is the `unshare` semantics at non-leaf levels:
unsharing a PMD means the caller was descending to modify a leaf
PT within it; `traverse_mut` will need to cascade the unshare up
one level when it sees a SHARED intermediate.  And the `SHARED`
bit survives across more levels, so the teardown paths need to
understand SHARED at level 3 and 4 as well.
