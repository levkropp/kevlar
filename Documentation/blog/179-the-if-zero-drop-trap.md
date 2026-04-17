## Blog 179: the IF=0 drop trap — stale TLB, VecDeque corruption, deferred teardown

**Date:** 2026-04-17

After blog 178 left two intermittent crash modes open — a WAITQ `Arc<Process>`
whose pointer field got zeroed, and a kernel stack with a zeroed return
address — this session chased both down to one underlying mechanism: the
TLB shootdown that protects freed memory was being skipped because the
drop site held IF=0, and the IF=0-guard path in `flush_tlb_for_teardown`
silently fell back to bump-gen-only.

## Catching the corruption in the act

We added a range check to `wake_all` before the `resume()` loop:

```rust
for process in &waiters {
    let raw = Arc::as_ptr(process) as usize;
    if raw < 0xffff_8000_0000_0000 {
        // dump all slots, then panic
    }
    process.resume();
}
```

`Arc::as_ptr` reads the pointer field of the `Arc` without dereferencing
it — exactly what we needed. On the next crash the dump arrived:

```
WAITQ CORRUPT: queue=0xffff800002ca0ac8 bad_arc[1]=0x11 of 16
  waiters[0]  = 0xffff80003d3f9110   <- kernel VA, valid
  waiters[1]  = 0x11                  <- 17 decimal
  waiters[2]  = 0x264d5010            <- physical-looking
  waiters[3]  = 0x11
  waiters[4]  = 0x2679a010
  waiters[5]  = 0x11
  waiters[6]  = 0x2679d010
  waiters[7]  = 0x11
  waiters[8]  = 0xffff80003d145010   <- valid again
```

Three things jumped out:

1. The queue address `0xffff800002ca0ac8` was consistent across crashes —
   resolving the symbol pointed to `POLL_WAIT_QUEUE`, the one the timer
   ISR pokes on every tick to resume poll/epoll/select waiters.
2. The corruption wasn't zero bytes — it was a structured pattern of
   three consecutive 16-byte `(0x11, some_paddr_ending_0x010)` blocks,
   each overwriting two `Arc<Process>` slots.
3. Slots `[0]` and `[8+]` were untouched.

The pattern is user data. Something was writing 16-byte pairs where it
thought it had a legitimate 48-byte object; those writes landed on the
middle of the WaitQueue's `VecDeque` backing buffer.

## Why the earlier fixes didn't catch this

Blog 178 had already added `tlb_remote_flush_all_pcids()` (INVPCID
type=3 broadcast) to `PageTable::flush_tlb_remote`, and
`flush_tlb_for_teardown` bumped the global PCID generation at
`Vm::Drop`. Those protect the *common* case: a `munmap` or a process
exit path running with IF=1 on a regular syscall. The IPI goes out,
every CPU's TLB gets invalidated, the freed pages are safe.

But `flush_tlb_for_teardown` has a guard:

```rust
fn flush_tlb_for_teardown() {
    bump_global_pcid_generation();
    if super::interrupts_enabled() {
        super::apic::tlb_remote_flush_all_pcids();
        flush_all_pcids();
    }
}
```

With IF=0, the IPI can't be sent — spin-waiting for ACK with IF=0 would
deadlock against another CPU also doing a TLB shootdown (we fixed
exactly that timer-death deadlock in commit 653007c). So the fallback
is to only bump the generation and let each CPU flush its own TLB on
its next context switch.

The problem: between "bump the gen" and "CPU A's next context switch",
the freed pages are live. If a stale TLB entry on CPU A points a user
VA to one of those pages, user writes through that VA reach the page's
new owner.

## Who drops with IF=0?

Arc<Process> gets dropped in several places:

- When a process exits, the last ref typically lives in
  `EXITED_PROCESSES`, drained by `gc_exited_processes` in the IRQ
  bottom half.
- When a wait queue is woken, each waiter is `resume()`'d and the
  `Vec<Arc<Process>>` drops at the end of the function.
- When a parent is reaped, its `children: Vec<Arc<Process>>` drops.
- When a thread is context-switched off, the outgoing `Arc<Process>`
  drops (unless something else holds it).

The IRQ bottom half runs with IF=0. `gc_exited_processes` is called
from `interval_work` in the bottom half, still under IF=0. So every
time gc ran and the reap dropped a Vm whose last user page had been
recently unmapped, the teardown fell through the IF=0 fallback.

## Fix 1: hold your Arcs, release the lock, then drop

```rust
pub fn gc_exited_processes() {
    let to_drop: Vec<Arc<Process>> = {
        let mut exited = EXITED_PROCESSES.lock();
        if exited.is_empty() { return; }
        core::mem::take(&mut *exited)
    }; // EXITED_PROCESSES lock released

    let was_if_off = !interrupts_enabled();
    if was_if_off { enable_interrupts(); }
    drop(to_drop);
    process_deferred_vm_teardowns();
    if was_if_off { unsafe { asm!("cli"); } }
}
```

Two pieces: the `mem::take` is there so the Arc drops don't run while
the `EXITED_PROCESSES` SpinLock is held (its Drop would re-enter on
panic paths). The `enable_interrupts()` is there so `Vm::Drop`'s IPI
can actually send.

## Fix 2: when you can't enable IF, defer

The other drop sites (wake_all's waiters Vec, children Vec) can't
freely re-enable interrupts — they run inside a spinlock-holding
caller, or in the IRQ top half itself. So we added a deferred list:

```rust
pub static DEFERRED_VM_TEARDOWNS: SpinLock<Vec<DeferredTeardown>> = ...;

impl Drop for Vm {
    fn drop(&mut self) {
        let kind = /* GhostForked | Forked | None (exec'd) */;
        if kind.is_none() { return; }

        if !interrupts_enabled() {
            let pml4 = self.page_table.pml4();
            self.page_table.clear_pml4_for_defer(); // no double-free
            DEFERRED_VM_TEARDOWNS.lock_no_irq().push(
                DeferredTeardown { pml4, kind: kind.unwrap() },
            );
            return;
        }

        // normal IF=1 path
        match kind.unwrap() {
            Forked => self.page_table.teardown_forked_pages(),
            GhostForked => self.page_table.teardown_ghost_pages(),
        }
    }
}
```

`gc_exited_processes` calls `process_deferred_vm_teardowns()` after
it's flipped IF on, so pending teardowns get completed before control
returns.

We had to add two tiny PageTable methods for this: `pml4()` to read
the address out, and `clear_pml4_for_defer()` to zero the field so
the original `PageTable`'s Drop path doesn't also free it. (Rust's
Drop semantics: the struct *will* be dropped; we just arrange for the
expensive work to happen elsewhere.)

## Fix 3: a seatbelt in wake_all

Even with both fixes, corruption from paths we haven't yet covered
would dereference a bad `Arc`. Rather than panic, we forget bad Arcs:

```rust
for process in waiters.into_iter() {
    let raw = Arc::as_ptr(&process) as usize;
    if raw < 0xffff_8000_0000_0000 {
        log::warn!("WAITQ CORRUPT: queue={:p} bad arc={:#x} — forgetting",
                   self, raw);
        core::mem::forget(process);
        continue;
    }
    process.resume();
}
```

`mem::forget` skips the Drop impl — a proper drop would dereference
the bad pointer and fault. The Process is leaked when this fires,
which is strictly better than a kernel panic during XFCE startup.

## How it tests

Before this session: 1/4 of runs deadlocked on `TLB_SHOOTDOWN_LOCK`,
rest crashed with `Process::resume+0xf4 CR2≈0x720`.

After blog 178's fixes: deadlock gone, ~3/4 of runs reach 3-4/4, one
in four still crashed with either a PT cookie panic or a `resume` on
a zeroed Arc.

After this session's IF=0 fixes: 6/8 of runs complete without a
kernel panic, PT cookie panic is gone, `resume`-on-zeroed-Arc is gone.
Two runs still hit a different symptom — `RIP=0 → RIP=2`, kernel stack
page's saved return address has been zeroed. Same flavor of bug (user
write landing on a reissued kernel page), but hitting a different
drop site we haven't covered yet.

## What's actually load-bearing

An instrumented counter revealed something unexpected: the
`IMMEDIATE` path count stays at **0**. Every single `Vm::Drop` sees
IF=0. The work is all going through the deferred path + the gc
drainer. What actually happens:

1. `gc_exited_processes` runs from the IRQ bottom half (IF=0).
2. It `mem::take`s EXITED_PROCESSES, releases the lock, enables IF.
3. `drop(to_drop)` starts iterating.
4. IF=1 means a pending timer IRQ can now fire — and it does.
5. The nested timer ISR calls `POLL_WAIT_QUEUE.wake_all()`.
6. `wake_all`'s waiters Vec drops at end of function, triggering
   `Vm::Drop` inside the ISR (IF=0).
7. `Vm::Drop` sees IF=0, stashes pml4, returns.
8. ISR returns. Back in gc.
9. gc calls `process_deferred_vm_teardowns()` with IF=1, which
   runs the deferred teardowns correctly (IPIs fire, TLBs flush).

So step 9 is doing all the real work. Removing either piece — the
IF=1 enablement or the deferred path — puts us back in the bug.

## What's next

Stability went from ~1/8 to 7/8 runs completing without kernel
panic. The one remaining crash hits the panic handler's own
unwinder (`gimli::parse_cfi`) — something panicked, the unwinder
crashed on bad metadata, masking the root cause. Likely still the
kernel-stack-RIP-zero symptom on a drop site we haven't covered —
the candidates: `children: Vec<Arc<Process>>` drop from
`Process::Drop`, the outgoing Arc in `switch()`'s `CURRENT.set`,
and the `prev` Arc in `switch()`'s `drop(prev)`. Audit those and
the remaining crash should fall.
