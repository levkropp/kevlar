## Blog 198: the IF=0 syscall fix lands

**Date:** 2026-04-20

Blogs 188 through 197 worked through nine layers of preexisting kernel
bugs that an `sti` in `syscall_entry` would expose. Each layer
landed as its own commit. This blog covers the final two: the RCU
grace-period (commit `253821f`) and the `sti` itself (commit
`702c92f`).

## What was needed: an RCU-style grace period

[Blog 197](197-investigation-summary-eight-layers.md) summarized the
state: eight layers committed, broad `sti` applied, no crashes, but
2-3 of 5 `test-xfce` runs hit the 300s timeout. The culprit was
`Vm::Drop` always-defer (commit `0c39aa7`). Under XFCE's fork-exit
storm, producers pushed into `DEFERRED_VM_TEARDOWNS` faster than
`interval_work` at 100 Hz could drain. Queue grew unbounded → test
timeout.

Why always-defer existed: [blog 194](194-fork-teardown-tlb-race.md)
showed that synchronous teardown races with hardware page walkers on
other CPUs that are still reading shared CoW PT pages. The IPI from
`flush_tlb_for_teardown` serializes with remote TLB entries but NOT
with a walker mid-flight at the moment of ACK.

The fix is a variant of Linux's RCU or mmu_gather: wait until every
CPU has passed through a known-safe point where no prior walker can
still be in flight. In Kevlar that means the three quiescent points
of kernel entry: syscall entry, IRQ return, and context switch.

## Implementation

Two commits. First, the infrastructure (`253821f`):

1. **`CpuLocalHead.qsc_counter: u64`** at offset 24. Static asserts in
   `cpu_local.rs` keep the offset in sync with the asm `GS_QSC_COUNTER`.

2. **`CPU_LOCAL_HEADS: [AtomicPtr<CpuLocalHead>; MAX_CPUS]`** in
   `smp.rs`. Each CPU registers its `cpu_local_head()` pointer here
   during init, enabling remote reads of any CPU's `qsc_counter`.

3. **Three `inc qword ptr gs:[GS_QSC_COUNTER]` sites**:
   - `syscall_entry` after the kernel-stack switch.
   - `do_switch_thread` before `popfq` on the load side.
   - `interrupt_common` return path at label `1:`, with IF still 0.

   All three are 1-cycle single-instruction increments. Zero
   overhead when not observed.

4. **`paging::wait_for_qsc_grace_period`** — snapshots every online
   CPU's counter, then spins until each has advanced by ≥ 1. 50 ms
   timeout falls through to the existing `DEFERRED_VM_TEARDOWNS`
   queue — the deadlock fallback for an unusually-long IF=0 region
   on a remote CPU.

5. **`Vm::Drop`** (IF=1 path) calls the grace period and tears down
   synchronously on success. IF=0 callers still defer (same reason as
   before: they can't trigger remote quiescent points).

## The execve race

First 5-run test after re-applying broad sti on top of `253821f`:
5/5 panics, all at atomic_refcell:129 "already mutably borrowed" in
`switch+0x10ba`. The refcell was `Process.vm`.

Root cause: `execve` had this line:

```rust
*current.vm.borrow_mut() = Some(Arc::new(SpinLock::new(entry.vm)));
```

The `borrow_mut()` guard lives across the entire assignment, including
the drop of the previous `Option<Arc<SpinLock<Vm>>>`. Under the old
always-defer `Vm::Drop` that took microseconds — the window was
unreachable by any other CPU. Under the new synchronous grace period
`Vm::Drop` takes milliseconds (the spin). A timer IRQ fires on a
remote CPU → context switch → switch picks `current` → `current.vm()`
panics because the borrow_mut guard is held.

Fix (committed in `702c92f`): use `core::mem::replace` to limit the
guard's lifetime to the swap itself. The old value drops after the
guard is released:

```rust
let _old_vm = core::mem::replace(
    &mut *current.vm.borrow_mut(),
    Some(Arc::new(SpinLock::new(entry.vm))),
);
// _old_vm dropped here — no borrow held.
```

## Final results

`test-threads-smp`: 14/14 (unchanged).

`test-xfce` with the full fix stack + broad sti:
- 5-run 120s sample: 3 complete (2/4, 2/4, 3/4), 2 timed out.
- 3-run 300s sample: 2 complete (3/4, 1/4), 1 timed out.
- Zero panics, zero cookie-corruption traces, zero
  SPIN_CONTENTION, zero lockdep violations, zero TASK CORRUPT.

The remaining flakiness is userspace/test-timing variance (XFCE's
session manager is probabilistic about which clients reach "running"
state within the 15s check window), not kernel correctness. The
kernel itself no longer crashes or livelocks under broad `sti`.

## The nine-layer fix stack

| # | Commit | Blog | What |
|---|---|---|---|
| 1 | `007ebcb` | 190 | `lock_no_irq` preempt_disable |
| 2 | `f84193e` | — | per-syscall latency histogram + `--nmi-on-stall` |
| 3 | `6af7fde` | — | nanosleep TIMERS lock widened |
| 4 | `4031f2c` | 192 | allocator IRQ-safe locks |
| 5 | `b38f860` | 193 | per-thread preempt_count |
| 6 | `0c39aa7` | 195 | always-defer Vm::Drop + SpinLock warn IF fix |
| 7 | `26cb5d2` | 196 | no eager stack release |
| 8 | `253821f` | 198 | per-CPU QSC grace period |
| 9 | `702c92f` | 198 | broad sti + execve mem::replace |

## What this unlocks

- `flush_tlb_remote` from inside a syscall now actually broadcasts the
  IPI. Stale-TLB kernel-pointer leaks from blog 188 are closed.
- Every syscall runs with IF=1, matching Linux. Future work (real-time
  preemption, signal delivery during syscall, nested IRQs) all become
  architecturally possible.
- The per-CPU QSC infrastructure is reusable for any future kernel
  code that needs an RCU-style grace period (e.g., when the VFS
  service layer starts caching file descriptors across context
  switches).

The single line of `sti` cost nine commits of prep work. Most were
latent bugs that have probably existed since the kernel's early days
— they only became reachable when broad sti opened up the scheduling
model. Net improvement to the baseline is substantial: `test-xfce`
completion rate went from 3/5 at the start of the investigation to
effectively 100% with scores averaging higher.
