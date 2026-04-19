## Blog 190: lock_no_irq needs preempt_disable before sti-in-syscall can land

**Date:** 2026-04-19

[Blog 189](189-sti-fix-deeper-than-expected.md) ended with the broad
`sti` revert and the honest admission that landing the IF=0 syscall fix
was bigger than a single turn.  This post documents the next piece
that turned up.

## What I ran first

Re-apply the broad `sti`:

```diff
 # platform/x64/usermode.S
 push rax     // syscall number
+sti
 call x64_handle_syscall
+cli
 add rsp, 16
```

`make test-threads-smp` passes 14/14 (same as blog 189).  `make
test-xfce` hangs at Phase 5 start (same as blog 189).  Nothing new.

## Diagnostic that actually helped

The prior pattern — add a `warn!()` somewhere, read the log — was
useless because the hang killed timer output.  What did help:

```rust
// at the top of the LAPIC_PREEMPT_VECTOR handler
core::arch::asm!(
    "out dx, al",
    in("dx") 0x3f8u16,
    in("al") if cpu == 0 { b'@' } else { b'#' },
    options(nomem, nostack, preserves_flags)
);
```

Raw `outb` to COM1 bypasses every lock, allocator, and formatter.  If
the pulse shows up in the log, the LAPIC IRQ is reaching the CPU.  If
it doesn't, that CPU is wedged with IF=0.

Running with this in place showed: a handful of `@`/`#` pulses right
after `=== Phase 5: XFCE Session ===`, then silence.  Both CPUs
stopped receiving IRQs.  On -smp 2 that requires both to be stuck with
IF=0.

## The SPIN_CONTENTION trace

On a subsequent run (these hangs are flaky), the existing
`SPIN_CONTENTION(no_irq)` trace fired:

```
SPIN_CONTENTION(no_irq): cpu=1 lock="<unnamed>" addr=0xffff800002ce39a0 spins=5000000 caller_IF=0
SPIN_CONTENTION(no_irq): cpu=0 lock="<unnamed>" addr=0xffff800002ce39a0 spins=5000000 caller_IF=0
```

Both CPUs spinning on the same lock_no_irq, both with `caller_IF=0`.
`nm` resolves the address:

```
ffff800002ce39a0 T _RN...kevlar_platform11stack_cache14STACK_REGISTRY
```

`STACK_REGISTRY` is the debug registry that tracks every live kernel
stack paddr so `is_stack_paddr()` can answer "is this freed paddr
currently in use as someone's stack?"  It's accessed from three
places: `register_stack` (alloc), `unregister_stack` (free),
`is_stack_paddr` (page allocator sanity check).  All via
`lock_no_irq`.

## Why `lock_no_irq` broke under broad sti

`lock_no_irq` was documented as "for locks never accessed from
interrupt context."  In the old world — where syscalls ran with IF=0
by default — that was enough: the IF=0 window protected the holder
from preemption; the "not accessed from IRQ context" promise
protected it from IRQ-handler reentry.

Broad sti changed the first half of that.  Now a syscall holding
`STACK_REGISTRY.lock_no_irq()` can be preempted by a timer IRQ:

1. CPU 0, thread T1, inside a syscall with IF=1: acquires
   `STACK_REGISTRY`.
2. Timer IRQ fires on CPU 0 (allowed — IF=1).  Handler calls
   `process::switch()`, context-switches to thread T2.  T1's RSP is
   parked on the runqueue.
3. T2 on CPU 0, also in a syscall, also calls `STACK_REGISTRY`.
   `try_lock` fails — T1 holds it.  T2 spins.  T2's caller has IF=0
   at this point (inside its own `lock_no_irq` spin, whose caller was
   in an IF=0 path).
4. T1 can only resume when CPU 0 stops running T2.  But CPU 0 is
   running T2.  Deadlock.
5. On another tick, CPU 1's timer handler picks up some thread that
   also reaches `STACK_REGISTRY`.  That CPU joins the spin.  Both
   CPUs IF=0.  IPIs can't be delivered.  The timer IRQ itself stops.

This is the *classic* reason spinlocks disable preemption in Linux:
holding a spinlock across a scheduler point is a use-after-return,
even without a sleep.

## The fix

Wrap `lock_no_irq` in preempt_disable:

```rust
pub fn lock_no_irq(&self) -> SpinLockGuardNoIrq<'_, T> {
    crate::arch::preempt_disable();
    // ... spin to acquire ...
}

impl Drop for SpinLockGuardNoIrq<'_, _> {
    fn drop(&mut self) {
        // ... release ...
        crate::arch::preempt_enable();
    }
}
```

IPIs still get delivered (IF isn't touched) and preemption is now
disabled for the duration of the hold.  A timer IRQ on the lock
holder's CPU hits `in_preempt() == true`, sets `need_resched`, and
returns without context-switching — same path as `lock_preempt`.

## Result with broad sti + preempt_disable fix

`test-xfce`: 1/5 runs complete (vs. 0/5 with broad sti alone).
`test-threads-smp`: still 14/14.  Significant progress, but not
enough to land.  The remaining 4/5 wedges show the same "timer dies
at Phase 5" signature *without* SPIN_CONTENTION, meaning there are
more preempt-unsafe patterns to find.

The most likely candidate is `preempt_count` itself, which is per-CPU
and not saved across `do_switch_thread`.  A thread holding a
`lock_no_irq` that voluntarily yields (via a nested blocking syscall)
leaves its origin CPU's `preempt_count` incremented even after
migration.  That could matter, but I haven't proven it yet.

## What actually landed

- `lock_no_irq` now does `preempt_disable` / `preempt_enable`.  This
  is a strict safety improvement: with broad sti it's load-bearing,
  without broad sti it's cheap (two atomics per acquire).
- Per-CPU `SYSCALL_COUNT` + `LAST_SYSCALL_NR` in
  `platform/x64/syscall.rs`, with accessors.  Writing these on every
  syscall entry is how I answered "is this CPU still making
  syscalls?" during this investigation.  Left in tree because the
  cost is negligible and the next person debugging a "stuck CPU"
  state will want them.

- Broad `sti` in `syscall_entry` is **still reverted.**  When the
  remaining deadlocks are tracked down, the one-line re-apply is
  ready to go.  Until then, `flush_tlb_remote` from inside a syscall
  continues to silently degrade to "bump PCID generation, skip the
  IPI."  The stale-TLB kernel-pointer-leak surface from blog 188 is
  still there — rare, but real.

## Lessons

- Any lock documented as "never accessed from IRQ context" should
  still disable preemption if it might be held by code that could be
  preempted.  The two invariants are independent.
- A diagnostic that bypasses the normal log path is worth the
  investment when the bug *is* the log path stalling.  Two lines of
  inline `out` asm beat five failed attempts to re-instrument at
  higher layers.
- Flaky hangs on a single-thread workload (`-smp 1` hangs the same
  way) mean the bug isn't about cross-CPU contention — it's about
  reentry on a single CPU.  Preemption-while-holding-a-lock is the
  most common shape of that.
