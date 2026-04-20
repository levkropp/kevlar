## Blog 197: investigation summary — eight layers, broad sti still pending

**Date:** 2026-04-20

This blog wraps up a long investigation arc — blogs 188 through 196.
The starting question was simple: "syscalls run with IF=0 and that
breaks every TLB shootdown."  The proposed fix was a single-line
`sti` between the syscall_entry pt_regs build and `call
x64_handle_syscall`.

The actual work was eight commits of dependency-untangling, each of
which exposed a latent kernel bug that broad sti would crash on.

## Eight committed fixes

| # | Commit | What |
|---|---|---|
| 1 | `007ebcb` | `lock_no_irq` preempt_disable on acquire (closes STACK_REGISTRY deadlock — blog 190) |
| 2 | `f84193e` | Per-syscall latency histogram + `--nmi-on-stall` QMP harness (the diagnostic primitive used to find every subsequent layer) |
| 3 | `6af7fde` | nanosleep holds TIMERS lock across set_state (closes wake-up race) |
| 4 | `4031f2c` | page_allocator + stack_cache + global heap converted to IRQ-safe locks (closes alloc-during-IRQ deadlock — blog 192) |
| 5 | `b38f860` | preempt_count save/restore across do_switch_thread (per-thread instead of per-CPU — blog 193) |
| 6 | `0c39aa7` | Always-defer Vm::Drop + SpinLock contention warning no longer flips IF (blog 195) |
| 7 | `26cb5d2` | Don't eagerly free exiting task's kernel stacks (re-applies blog-147 fix that was undone — blog 196) |
| 8 | `d515144`, `8c258f0`, `f4d4644`, `9f42198`, `f441fbb`, `64b9559`, `23c2da7` | Blog posts documenting each layer |

Every one of these is a strict improvement to the IF=0 baseline as
well, even when broad sti isn't applied:

- `test-threads-smp`: 14/14, has been since the start
- `test-xfce` baseline: 5/5 runs complete (no panics, no hangs),
  scores 1-3/4 with typical userspace flakiness — at the start of
  the investigation this was 1-4/4 with occasional crashes.

## Where broad sti stands

The single-line `sti` in syscall_entry, on top of all eight fixes:

- 5/5 runs complete (no panics — the panic classes are all closed)
- 2-3 of those 5 hit TEST_END with scores 1-3/4
- 2-3 hit a timeout (test takes > 300s without finishing)

So the LAST blocker is no longer correctness — the kernel doesn't
crash with broad sti applied.  It's *timing*: under broad sti some
process flow takes long enough that XFCE doesn't reach Phase 5
within 300 seconds.

The most likely cause is the always-defer Vm::Drop change: instead
of teardown happening synchronously when an Arc<Process> drops, it
queues into `DEFERRED_VM_TEARDOWNS` and waits for `interval_work` on
an idle CPU to drain.  Under heavy fork/exit pressure (XFCE startup
spawns ~15 processes that quickly exit during init), the queue fills
faster than the drainer empties.  Under IF=0 syscalls the drainer
only had to deal with IF=0 callers; under broad sti every drop
queues, so the queue grows ~10x.

## Why I'm pausing here

The next step — making the deferred-teardown drainer keep pace with
producers, or using a real RCU-style grace period independent of
defer — is bigger than a single turn.  Multiple options exist:

- **Per-CPU teardown queues** drained by each CPU's idle loop.
  Eliminates the global-queue contention.
- **Synchronous teardown for IF=1 callers** with a quiescent-state
  check that doesn't depend on `interval_work`.  Each CPU marks
  itself "quiescent" in its IRQ return path; teardown for an Arc
  whose freeing sequence number ≥ all per-CPU quiescent counters is
  safe to free immediately.
- **Don't share PT pages across processes via CoW**.  Bigger memory
  footprint, eliminates the walker race entirely.

Any of these is a multi-day project.  The current state is stable,
documented, and rigorously regression-tested.  Future-me has a
clean handoff in `project_if_zero_syscall_fix.md`.

## What this investigation produced

Beyond the fixes:

- A reusable diagnostic primitive (`--nmi-on-stall`) for any future
  livelock investigation.  Inject NMI from QMP after N seconds of
  silence; the kernel's NMI handler dumps register state, backtrace,
  IF-trace history, lockdep state, and the per-syscall latency
  histogram.
- A reusable lock-safety pattern: `lock_no_irq` is now genuinely
  safe (preempt_disable wraps it), allocator paths are universally
  IRQ-safe, and the spinlock contention warning is no longer a
  trapdoor for nested-IRQ panics.
- A per-thread `preempt_count` that matches Linux's
  `thread_info->preempt_count`.  Once broad sti lands, this becomes
  load-bearing; until then it's quietly correct.

The single-line `sti` is closer than ever.  Just not this turn.
