## Blog 191: the broad-sti failure is a livelock, not a deadlock

**Date:** 2026-04-19

[Blog 190](190-lock-no-irq-preempt-disable.md) closed the
STACK_REGISTRY deadlock by adding `preempt_disable` to
`lock_no_irq` and reported "broad sti still hangs test-xfce ~4/5
runs, with the `timer dies at Phase 5` signature but no
SPIN_CONTENTION."  This post is the next slice: the remaining
hang isn't a hang, and the new diagnostic that proves it.

## Better diagnostic: raw-COM1 from idle

The existing `TICK_HB` heartbeat is printed from inside
`handle_timer_irq`.  If that function stalls, `TICK_HB` stops
producing output — but so does everything else downstream.  The
test log goes silent, and it's indistinguishable from "timer IRQ
doesn't fire at all" and "timer IRQ fires but hangs inside the
handler" and "handler finishes but the log path is wedged."

New probe: emit `<I cpu=X sc0=N lsn0=N sc1=N lsn1=N>` every 20
iterations of `interval_work` (the per-CPU idle loop body).  The
emission goes through a raw `out dx, al` to COM1 — no locks, no
allocation, no dependency on the log backend.  Fields:

- `sc0` / `sc1`: per-CPU syscall counters
- `lsn0` / `lsn1`: last syscall number per CPU

If idle is running, this fires.  If idle is running, the kernel
isn't wedged in a single long lock or IF=0 loop — it's just busy
somewhere else.

## What the probe showed

With broad `sti` applied (so the residual bug is exposed) and a
300-second test-xfce run:

- `TICK_HB` stops after `tick=200` (2 seconds of boot).  As
  expected from blog 189.
- The idle diag **keeps firing** — 22 emissions over 90 seconds.
  So `interval_work` runs.  So idle runs.  So something is waking
  idle.  The system isn't wedged.
- `sc0` advances very slowly: 28 → 43 → 61 → … → 97 across the
  whole 300-second timeout.  That's **0.3 syscalls/sec**, vs
  5 000/sec during normal Phase 5 execution.
- `lsn0` cycles through ordinary Phase 5 syscalls:
  `mprotect(10)`, `execve(59)`, `getuid(102)`, `stat(4)`.  No
  single syscall is stuck — each one completes, but each takes
  seconds instead of microseconds.

So the system isn't deadlocked.  It's livelocked: forward
progress exists, but at a rate 1 000× too slow for the test to
finish before its timeout.

## What could cause a 1 000× slowdown?

The obvious sources of overhead in broad-sti mode are:

1. Actual TLB IPIs instead of the IF=0 bump-gen fallback.  At
   ~1 µs per IPI round-trip on KVM, even a busy mprotect would
   add only microseconds.
2. Additional context switches from timer preemption inside
   syscalls.  A context switch is tens of microseconds.  Even at
   100 Hz that's a few ms/sec of overhead, not 1 000×.
3. `preempt_enable` on every `lock_no_irq` drop now
   potentially triggers `switch()` if `need_resched` is set.
   With broad sti, `need_resched` gets set on every timer tick
   that hits a `preempt_count > 0` window.  The next `lock_no_irq`
   drop then takes the slow path.  There are ~147 `lock_no_irq`
   call sites; hot paths hit several per operation.

None of these individually explain 1 000×.  The number feels
more like a pathological livelock — two or more threads trading
off progress without completing their work, or a retry loop
that resets state each time it's preempted.

Without more instrumentation I can't tell which.  The honest
next move is to keep broad sti reverted, log the finding, and
come back with a better toolchain.

## What the probe is worth keeping

The raw-COM1 idle diag has been removed from the tree (it
interleaves with normal log output and would pollute strace).
But the **per-CPU SYSCALL_COUNT + LAST_SYSCALL_NR statics**
from blog 190 are still in tree at `platform/x64/syscall.rs`,
exposed via `kevlar_platform::arch::syscall_counter_read(cpu)` /
`last_syscall_nr_read(cpu)`.  The next investigator can wire
them into any temporary dump path without re-adding the
atomics themselves.

## Status

`test-xfce PROFILE=balanced`: 5/5 runs complete at the current
committed baseline (broad sti reverted, `lock_no_irq`
preempt_disable kept).  Typical score 2-4/4, best 4/4.
`test-threads-smp`: 14/14.

The IF=0 TLB-shootdown degradation from blog 188 is still a
real but silent issue — kernel pointers can leak into user
heap pages via stale TLBs.  Re-applying broad sti is the fix;
landing it is the open problem.
