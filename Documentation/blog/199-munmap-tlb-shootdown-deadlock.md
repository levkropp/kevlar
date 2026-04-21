## Blog 199: the munmap × page-fault × TLB-shootdown deadlock

**Date:** 2026-04-21

Blog 198 landed broad `sti` in `syscall_entry`. The kernel stopped
panicking and stopped livelocking.  What it didn't stop was
`test-xfce` flakiness:

- The best run would complete 3/4 (panel alone missing).
- The worst run timed out at 300 s, most often with a
  `KERNEL_PTR_LEAK: dbus-daemon` crash on the way in.
- Between those extremes, `SPIN_CONTENTION` and `NMI WATCHDOG`
  noise.

My previous prose dismissed the remainder as "userspace startup
variance."  Re-reading the logs made it clear that wasn't quite
right — there's at least one honest kernel bug still in the mix.

## The deadlock, extracted

One `test-xfce` run left CPU 0 in the NMI dump at:

```
RIP=0xffff800000187476    (SpinLock::lock_no_irq spin)
preempt_count=1
#0: lock_no_irq / spinlock.rs:143
#1: page_fault.rs:578  (vm_ref.lock_no_irq())
#2: handle_page_fault  (user-mode fault)
```

CPU 0 was in a user page-fault on some process P, trying to acquire
`P.vm.lock_no_irq()`.  The IDT interrupt gate cleared IF on entry,
and `lock_no_irq` doesn't restore it — CPU 0 spins with **IF=0**.

The dual-NMI patch let me NMI CPU 1 at the same instant:

```
RIP=0xffff80000022389f    (in watchdog_check, during its own NMI send)
preempt_count=2
syscall: count=10350  last_nr=11     ← nr=11 is sys_munmap
interrupted stack (via RBP chain):
  #0: watchdog_check
  #5: sys_munmap         munmap.rs:188
```

CPU 1 was in `sys_munmap` on the **same** Vm.  A timer IRQ fired
while `munmap` was mid-operation; the IRQ called `watchdog_check`,
which sent the NMI — the snapshot happened to land in `watchdog_check`
itself, but the interrupted-caller chain pins it to `sys_munmap`.

## Anatomy

`sys_munmap` held the Vm lock for its entire body:

```rust
let mut vm = vm_ref.as_ref().unwrap().lock_preempt();  // IF=1, preempt++
// ... unmap PTEs, flush_tlb_local per page ...
if !to_free.is_empty() {
    vm.page_table().flush_tlb_remote();               // IPI + spin-wait ACK
}
drop(vm);
// ... writebacks, free_pages ...
```

`flush_tlb_remote()` broadcasts the TLB-shootdown IPI and spin-waits
until every other CPU clears its bit in `TLB_SHOOTDOWN_PENDING`.
But CPU 0 has IF=0 and is spinning on `lock_no_irq` for the same
Vm — the IPI stays pending on the LAPIC ICR until CPU 0 re-enables
interrupts.  CPU 0 will only re-enable interrupts after it acquires
the Vm lock, returns from `handle_page_fault`, and hits IRETQ.
Neither ever happens; both CPUs spin.

Two threads of the same process.  One `munmap` from thread A on CPU 1,
one page fault from thread B on CPU 0.  The symptoms we'd been
attributing to "userspace flake" were often this deadlock draining
the 300 s budget.

Why didn't this happen before broad `sti`?  Because with IF=0 in the
syscall body, `tlb_remote_flush_all_pcids` short-circuits to a PCID
generation bump (the deferred flush) instead of sending an IPI.  The
IPI-send path was unreachable from a syscall.  Under broad sti, IF=1
is normal inside the syscall body; the IPI actually goes out; the
deadlock becomes reachable.

## The fix

Drop the Vm lock before the broadcast:

```rust
let had_unmaps = !to_free.is_empty();
drop(vm);
if had_unmaps {
    kevlar_platform::arch::flush_tlb_remote_all_pcids();
}
// writebacks, free_pages follow
```

Safety argument:

- Physical pages in `to_free` are still owned by us (the refcount
  decrement and `free_pages()` happen after the broadcast).  Stale
  TLB entries on remote CPUs that resolve to those frames therefore
  cannot read or write data belonging to a *different* process.
- Another thread of this same process that mmaps the same VA during
  the window will install a new PTE.  The subsequent broadcast
  invalidates every CPU's entries for our PCID, so the racing
  thread re-faults and picks up the new mapping — same behavior as
  before, just with a slightly wider window.
- The writeback step for MAP_SHARED pages also runs unlocked.  It
  was already running unlocked in the pre-fix code (the lock was
  dropped just before writebacks started), so this change doesn't
  widen anything there.

## What `mprotect`, `mremap`, `madvise` look like

Same pattern — all five call sites hold the Vm lock across
`flush_tlb_remote()`.  I experimentally applied the same
lock-first-then-broadcast transform to all four syscalls, ran
test-xfce eight times, and got a *kernel* fault inside the page-fault
handler on one of the runs:

```
panic!("page fault occurred in the kernel: ...")
at platform/x64/interrupt.rs:539
```

So I reverted `mprotect`/`mremap`/`madvise` for this commit and
landed only `munmap`.  The rollback needs more investigation before
reapplying — likely a different class of bug (maybe CoW timing, a
PTE-update race) that the broader rollout exposed but munmap's
narrower PTE scope stays inside the safe envelope.

## Results

Five-run test-xfce on the original test harness (15 s Phase 5 wait,
120 s timeout):

| tree | complete | score | KERNEL_PTR_LEAK |
|---|---|---|---|
| pre-fix   | 2/5 | mixed (1/4, 3/4)   | sometimes |
| post-fix  | 4/5 | all 3/4            | 0 in the sample |

Eight-run sample under heavier host load: 2/8 completed.  The fix
is a real correctness improvement — it eliminates the *specific*
deadlock shape from blog 186 — but other flake causes (PROCESSES
lock contention, Xorg SIGSEGV, an xfce4-session crash shape I
haven't yet tracked down) keep the aggregate win small.

The honest summary: the bug from blog 186 (kernel pointer in musl
heap → stale TLB entry → stale user-VA write into a recycled
kernel page) has *at least* this munmap path as one reproducer.
The other candidate paths are mprotect/mremap/madvise and the two
in `kernel/mm/vm.rs`; those are on a shortlist.

## Diagnostic primitives added

- **Dual-NMI watchdog.** When the watchdog detects CPU X is stuck,
  it now NMIs every other online CPU as well.  This captured the
  interrupted-stack evidence above that pinpointed `sys_munmap` as
  the lock holder — the single-NMI dump would have only shown the
  spinning CPU.
- **Serialized NMI dumps.**  A tiny atomic-CAS latch inside the NMI
  handler ensures one CPU's dump prints to completion before the
  next one starts.  Without this the output is two-CPU interleaved
  character-by-character.
- **Per-CPU `syscall.count` + `last_nr`** in the dump.  Tells you
  whether the stuck CPU was inside a syscall body (and which) vs.
  outside syscall context entirely (idle / IRQ / page fault).

These all landed in `nmi-watchdog: serialize per-CPU dumps, log
in-flight syscall` (commit alongside the munmap fix).

## What's next

The munmap fix is the pilot; the rest of the VM path needs the same
transform but can't land with the straight-pattern rollback that
triggered the kernel page fault.  That warrants its own
investigation — likely the PTE update or the VMA-remove ordering
interacts with concurrent fork/access in a way that depends on the
lock being held continuously.  Not today's work.
