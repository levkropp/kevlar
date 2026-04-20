## Blog 195: always-defer Vm::Drop + SpinLock contention fix

**Date:** 2026-04-20

Two more layers off the broad-sti fix. Committed as `0c39aa7`; neither
actually lands broad sti, but both close bugs that were masking progress.

## Vm::Drop: always defer

The `DEFERRED_VM_TEARDOWNS` mechanism (blog 178) was previously only
taken when `Vm::Drop` ran with IF=0.  The reasoning: if IF=1 the TLB
shootdown IPI can broadcast and receive ACKs normally, so teardown can
proceed inline.  That reasoning was correct under IF=0 syscalls.

Under broad sti, it's wrong.  Blog 194 showed the failure mode: CPU A
drops the last Arc<Process> with IF=1, enters teardown immediately.
CPU B's hardware page walker is still traversing shared CoW PT pages
from the same Vm.  The IPI ACK means CPU B's TLB has been invpcid'd,
but the walker in flight at the moment of ACK can still write A/D bits
into a PT page that teardown has already put back in `PT_PAGE_POOL`.
Next `alloc_pt_page` hits `panic!("PT page cookie corrupted")`.

Fix: always defer.  `process_deferred_vm_teardowns` is drained from
`gc_exited_processes` (itself called from `interval_work` on the idle
path).  Between "push to DEFERRED" and "drain", every CPU passes
through at least one context switch or idle tick — which is a
quiescent point for the hardware walker (the walker restarts after an
IRQ return).  Crude RCU, but it's enough for every race I've seen so
far.

## SpinLock contention warning no longer flips IF

`SpinLock::lock()` had this:

```rust
if spins == SPIN_CONTENTION_THRESHOLD {
    asm!("sti");                     // temporarily enable IRQs
    log::warn!("SPIN_CONTENTION ...");
    asm!("cli");                     // restore
}
```

The comment said "re-enable interrupts briefly so we can print and so
the NMI/timer can fire."  Neither is actually necessary — `warn!` uses
its own `spin::Mutex`-backed printer, and the NMI path doesn't care
about IF.  What the flip *did* do was create a window where a timer
IRQ could fire, run `handle_timer_irq`, try to acquire TIMERS (rank
10), and trip lockdep because the outer SpinLock (rank 40) was still
recorded as held.  Under broad sti PROCESSES contention went up and
this path started firing routinely.

Fix: remove the sti/cli.  warn!() runs with IF=0 from the outer lock's
cli; the output still gets out fine.

## Status

Neither change lands broad sti, but:
- test-threads-smp: 14/14 (unchanged)
- test-xfce baseline: 3/3 runs complete, scores 3/4, 4/4, 2/4

5-run test with broad sti on top of these two commits:
- 3 complete (1/4, 3/4, 3/4 — usable scores)
- 1 timed out after 90s (not a panic — just slow XFCE startup)
- 1 panicked with `switch_thread BUG: saved_rsp_off=0xffffffffffffba80`
  — a freed-and-reissued kernel stack, unrelated to teardown

The fresh panic is a different problem: some code path still
references an exited thread's saved rsp after `release_stacks` has
returned its kernel_stack to the pool.  This is a preexisting latent
bug that broad sti's changed timing exposes, and closing it needs a
careful audit of wake-queue / futex / scheduler-queue entries for
exiting threads.

## Where this leaves the landing effort

Six layers now peeled, all committed:

| | |
|---|---|
| `007ebcb` | `lock_no_irq` preempt_disable |
| `f84193e` | histogram + --nmi-on-stall |
| `6af7fde` | nanosleep TIMERS lock widened |
| `4031f2c` | allocator IRQ-safety |
| `b38f860` | per-thread preempt_count |
| `0c39aa7` | always-defer Vm::Drop + SpinLock warn-no-IF-flip |

Broad sti in `syscall_entry` remains the single uncommitted line.
The remaining blocker is the freed-stack-still-referenced class of
bug, which is unrelated to TLB/IPI/preempt questions but also becomes
reachable only under broad sti's mid-syscall preemption.
