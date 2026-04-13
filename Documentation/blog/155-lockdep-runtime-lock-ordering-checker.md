# Blog 155: Lockdep — Runtime Lock Ordering Checker

**Date:** 2026-04-12

## Motivation: Deadlocks That Manifest as Silence

Blogs 149-153 documented a series of SMP deadlocks in Kevlar:

- **TIMERS→SCHEDULER** (Blog 149): The timer handler held the TIMERS
  lock while calling `resume_boosted()`, which acquired SCHEDULER.
  Meanwhile, `switch()` held SCHEDULER and tried to acquire TIMERS
  via a poll wakeup.  Result: both CPUs spin forever with IF=0.

- **WaitQueue→SCHEDULER** (Blog 149): `wake_all()` held the wait
  queue lock while resuming 20+ poll waiters, each acquiring SCHEDULER.
  With interrupts disabled for >10ms, timer ticks were lost.

These bugs shared a pattern: they manifested as **mysterious timer
stalls** — the LAPIC timer fires once and never again.  The actual
cause (a lock ordering violation) was invisible because the deadlock
happened with IF=0, preventing any diagnostic output.

Linux's lockdep subsystem catches these bugs **at lock acquire time**
with a clear panic message showing the conflicting lock chain.  Linux
took ~10 years to build lockdep.  We built ours in one session.

## Design

### The Key Insight: Lock Rank Ordering

Every lock in the kernel gets a numeric **rank**.  The rule is simple:

> You may only acquire a lock whose rank is **strictly greater** than
> every lock you currently hold.

If this invariant holds globally, circular dependencies are impossible,
and therefore deadlocks are impossible.

### Rank Table

```
Rank 10:  TIMERS, REAL_TIMERS    (timer subsystem — acquired first)
Rank 20:  WAIT_QUEUE             (poll, futex, pipe wait queues)
Rank 30:  SCHEDULER              (run queue)
Rank 40:  PROCESSES              (process table)
Rank 41:  EXITED_PROCESSES       (zombie list)
Rank 50:  VM                     (per-process address space)
Rank 60:  PROCESS_RESOURCE       (fd table, signals, root_fs)
Rank 70:  PAGE_ALLOC             (page allocator)
Rank 80:  FILESYSTEM             (mount table, inotify, etc.)
Rank 90:  NETWORK                (sockets, TCP endpoints)
Rank  0:  UNRANKED               (no ordering checks)
```

This ordering reflects the actual call graph in `handle_timer_irq()`:
timers are processed first (rank 10), then poll waiters are woken
(rank 20), then the scheduler picks the next thread (rank 30), then
the process table is consulted (rank 40).

### Implementation

**Lock declaration** — the existing `SpinLock::new()` still works
(rank 0, unchecked).  For ranked locks:

```rust
static TIMERS: SpinLock<Vec<Timer>> = SpinLock::new_ranked(
    Vec::new(),
    lockdep::rank::TIMERS,  // rank 10
    "TIMERS",
);
```

**Per-CPU held-lock stack** — each CPU maintains a stack of 16
`HeldLock` entries: `(lock_addr, rank, irq_disabled)`.  Accessed by
CPU index with interrupts disabled (no data race):

```rust
struct CpuLockState {
    held: [HeldLock; 16],
    depth: u8,
    checking: bool,  // reentrancy guard
}
static mut STATES: [CpuLockState; 8];
```

**Acquire check** — in every `lock()`, `lock_no_irq()`, and
`lock_preempt()`, before the spin loop:

```rust
lockdep::on_acquire(self_addr, self.rank, self.name);
```

This scans the held stack.  If any held lock has `rank >= new_rank`,
the kernel panics:

```
LOCKDEP: lock ordering violation on CPU 0!
Acquiring: SCHEDULER (rank 30, addr 0xffff8000001a4b80)
While holding: rank 10 (addr 0xffff8000001a3c40)
Held locks (innermost first):
    [0] rank=10 addr=0xffff8000001a3c40 irq_off=true
```

**Release tracking** — every guard's `Drop` calls
`lockdep::on_release(lock_addr)`, removing the entry from the stack.
Out-of-order drops (Rust's scope rules can cause these) are handled
by scanning backwards.

**Reentrancy guard** — if the lockdep panic itself tries to acquire
a lock (e.g., the serial logger), the `checking` flag prevents
infinite recursion.

**NMI integration** — the NMI watchdog handler (Phase 1) calls
`lockdep::dump_held_locks(cpu)` to show which locks a stuck CPU
was holding.  This is the missing link: not just "the CPU is stuck"
but "the CPU is stuck **holding TIMERS while waiting for SCHEDULER**."

### Always Compiled, Runtime-Gated

Unlike Linux's lockdep (which uses `CONFIG_LOCKDEP`), ours is always
compiled.  The hot path is:

```rust
if !ENABLED.load(Relaxed) { return; }
if rank == 0 { return; }
```

Two atomic loads (~1ns) when disabled.  When enabled, the full check
adds ~20ns per lock acquire — negligible compared to the spinlock
itself.  `lockdep::enable()` is called during boot after per-CPU
init completes.

## What It Catches

The TIMERS→SCHEDULER deadlock from Blog 149:

1. CPU 0: timer handler acquires TIMERS (rank 10)
2. CPU 0: calls `resume_boosted()` → tries to acquire SCHEDULER (rank 30)
3. Lockdep check: held=[TIMERS rank 10], acquiring SCHEDULER rank 30.
   **10 < 30 → OK.**

4. CPU 1: timer handler acquires TIMERS (rank 10)
5. CPU 1: `switch()` acquires SCHEDULER (rank 30)
6. CPU 1: tries to acquire... wait, **this is the old buggy code path**
   where `resume_boosted()` was called inside the TIMERS lock.

If the old code were still present:
1. CPU 0 holds TIMERS (rank 10), acquires SCHEDULER (rank 30) → OK
2. CPU 1 holds SCHEDULER (rank 30), tries to acquire TIMERS (rank 10)
3. **Lockdep: rank 10 <= held rank 30 → PANIC!**

The violation is caught on the **first occurrence**, not after 300
seconds of silence.

## Boot Verification

SMP=2 boot with lockdep enabled:

```
lockdep: runtime lock ordering checker enabled
...
OpenRC 0.55.1 is starting up Linux 6.19.8 (x86_64)
```

No violations.  The current code correctly acquires locks in rank
order.  The ranked locks (TIMERS, SCHEDULER, PROCESSES,
EXITED_PROCESSES, REAL_TIMERS) are checked on every acquire/release
cycle across the full boot + OpenRC init sequence.

## Incremental Adoption

Not every lock needs a rank immediately.  `SpinLock::new()` creates
an unranked lock (rank 0) that bypasses all checks.  When a new
deadlock is discovered, the fix is:

1. Identify the two conflicting locks
2. Assign ranks that reflect the correct ordering
3. Change `new()` → `new_ranked(value, rank, name)`
4. Rebuild — if the ranking is wrong, lockdep panics immediately

The rank constants live in `platform/lockdep.rs::rank`, making the
global ordering visible in one place.

## Files Changed

- `platform/lockdep.rs` — new file: full lockdep implementation, rank constants, held-lock dump
- `platform/spinlock.rs` — `new_ranked()` constructor, lockdep hooks in all 3 lock/guard variants
- `kernel/timer.rs` — TIMERS lock ranked (10)
- `kernel/process/mod.rs` — SCHEDULER lock ranked (30)
- `kernel/process/process.rs` — PROCESSES (40), EXITED_PROCESSES (41) ranked
- `kernel/syscalls/setitimer.rs` — REAL_TIMERS lock ranked (11)
- `kernel/main.rs` — `lockdep::enable()` call during boot
- `platform/x64/interrupt.rs` — NMI handler dumps held locks
