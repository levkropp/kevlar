## Blog 194: fork-teardown TLB race under broad sti

**Date:** 2026-04-20

Continuing to peel blocking issues before broad `sti` can land in
syscall_entry.  With the full fix stack from blogs 190-193 in tree,
re-applied broad sti and ran test-xfce 5 times: 1 complete, 1 hang,
3 panics.  All three panics hit the same site:

```
[PANIC] CPU=0 at platform/x64/paging.rs:32
PT page 0x3857d000 cookie corrupted: 0x0 (expected 0xbeefca11deadf00d)
    alloc_pt_page+0x3ca
    duplicate_table+0x1c
    duplicate_table+0x153  (recursive)
    duplicate_table+0x153
    duplicate_table+0x153
    PageTable::duplicate_from
    Process::fork
    sys_fork
```

The pool of recycled PT pages contains a page whose cookie has
been zeroed out.  PT_PAGE_POOL is populated only by
`free_pt_page` which stamps the cookie just before pushing, so
the corruption happened after the push and before the pop.  That
means *something is writing to the page while it sits in the
pool*.

## The teardown race

`Vm::Drop` calls `teardown_forked_pages` (or
`teardown_ghost_pages`), which calls `flush_tlb_for_teardown`
before iterating the page tables.

```rust
fn flush_tlb_for_teardown() {
    bump_global_pcid_generation();
    if super::interrupts_enabled() {
        super::apic::tlb_remote_flush_all_pcids();
        flush_all_pcids();
    }
}
```

The deferred-teardown machinery (`DEFERRED_VM_TEARDOWNS`, blog 178)
guarantees this function is reached with IF=1 when the original
Vm::Drop would have run with IF=0.  So the `if` branch does
actually run — IPIs fire, remote CPUs invpcid.

**But the IPI ACK doesn't mean the remote CPU's hardware page
walker has stopped using stale entries.**  Here's the specific
race:

1. CPU 0, mid-syscall (IF=1 under broad sti), drops the last
   Arc<Process> for a forked-and-then-exec'd child.
2. Vm::Drop fires with IF=1 → runs teardown immediately (no defer).
3. `flush_tlb_for_teardown` sends IPI, waits for ACK from CPU 1.
4. CPU 1 was in the middle of `duplicate_table` on the parent's
   VM — which *shares PT pages with the forked child* under CoW.
   Its hardware walker is actively reading the soon-to-be-freed
   PT page.  IPI handler runs in-between the walker's reads; it
   acks, runs invpcid.  The walker's next TLB lookup misses and
   re-walks — but the paging.rs code then *frees* that PT page
   milliseconds later, and a subsequent `alloc_pt_page` on any CPU
   hands it right back to `duplicate_table`, which overwrites the
   cookie and fills it with fresh PTEs.
5. Meanwhile CPU 1's hardware walker's re-walk writes A/D bits
   into entries of the (now freshly-zeroed-and-recycled) page,
   corrupting it.

The `PT page 0x... cookie corrupted` panic is the pool's guard
catching one such race on the POP side, before the caller uses a
poisoned page.  With IF=0 syscalls (the current committed state),
mid-syscall Vm::Drop is impossible because Arc refcount work
doesn't interleave with hardware walkers from other CPUs operating
on the same address space — nothing else is actively walking it
because the only way another CPU could be walking it is via fork
CoW on the parent, which is synchronous under IF=0.

## Partial fix that wasn't enough

I tried always doing the local flush regardless of IF state:

```rust
fn flush_tlb_for_teardown() {
    bump_global_pcid_generation();
    flush_all_pcids();          // always
    if super::interrupts_enabled() {
        super::apic::tlb_remote_flush_all_pcids();
    }
}
```

5-run: 3 complete (scores 1-3/4), 1 hang, 1 panicked with a
different symptom — a #GP on compare_exchange on a PTE pointer
inside `try_map_user_page_with_prot`, reached via unix socket
write's usercopy fault.  Different shape, same root cause
category: some intermediate PT page was freed out from under a
traverse in progress.

The correct fix is fundamentally harder than a single line:
fork's CoW PT-page sharing + broad sti + concurrent teardown is
a three-way race that requires either:

(a) **Never free PT pages that any CPU's CR3 transitively
   references.** Track a per-PT-page CPU refcount; wait for it to
   reach 0 before freeing. Expensive.

(b) **RCU-style grace period.** After teardown, put freed PT pages
   on a "pending free" list; return them to the pool only after
   every CPU has gone through at least one quiescent state
   (interrupt or context switch). Matches Linux's mmu_gather.

(c) **Serialize fork teardown against all current page-walks.**
   Acquire a per-VM reader/writer lock where traverse takes a read
   lock and teardown takes the write lock. High contention but
   correct.

Under the current IF=0 syscall model, option (d) implicitly holds:
no syscall can yield to another mid-walk, so within a single
"syscall" of the kernel's runtime, PT pages are stable.  Broad sti
breaks that invariant — the right fix is (b) or (c), not just one
more flush.

## Status

- Partial-fix revert: paging.rs and usermode.S restored to their
  committed state.  Baseline from `b38f860` preserved.
- `test-threads-smp`: 14/14.  `test-xfce` baseline: 3/3 complete,
  scores 4/4, 4/4, 3/4.
- Broad sti still not landed.  The remaining work is a page-table
  teardown safety primitive — not a deadlock-peeling iteration.

The five landed fixes (lock_no_irq preempt_disable, allocator IRQ
safety, nanosleep TIMERS lock widening, per-thread preempt_count,
diagnostic toolchain) are all strict improvements that reduce the
surface area but don't close this final race class.  A proper RCU
grace-period scheme for freed PT pages is the next piece of work
and is bigger than a single turn.
