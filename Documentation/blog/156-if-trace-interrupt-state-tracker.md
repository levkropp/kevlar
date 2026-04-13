# Blog 156: IF-Trace — Interrupt Flag State Tracker

**Date:** 2026-04-12

## The Missing Piece: *Where* Did IF Get Stuck?

Phase 1 (NMI watchdog, Blog 154) tells us **that** a CPU is stuck.
Phase 2 (lockdep, Blog 155) tells us **which locks** it's holding.
But neither answers the critical question: **what specific instruction
or lock operation set IF=0 and never restored it?**

On x86-64, interrupts are disabled by:
- `cli` — explicit in assembly
- Interrupt gates — hardware clears IF on entry
- `SYSCALL` — FMASK (IA32_FMASK=0x0700) clears IF
- `SpinLock::lock()` — saves RFLAGS then cli
- `popfq` / `iretq` — restores RFLAGS (may set IF=0 if saved value had IF=0)

And re-enabled by:
- `sti` — explicit in assembly
- `iretq` — restores RFLAGS (IF=1 if user mode)
- `sysret` — restores RFLAGS from R11
- `SpinLockGuard::drop()` — restores saved RFLAGS
- `popfq` — restores RFLAGS (IF=1 if saved that way)

The LAPIC timer bug (Blog 154) results in IF=0 forever.  Something
in the interrupt handler or context switch path clears IF and never
restores it.  With dozens of lock acquires and context switches per
timer tick, finding the exact transition requires a recorder.

## Design: Per-CPU Ring Buffer

Each CPU maintains a ring buffer of 256 entries (4KB), recording
every IF-changing event:

```
Entry (16 bytes):
┌──────────┬──────────┬───────┬──────────┬─────┐
│  TSC (8) │ src (4)  │ ev(1) │ IF_after  │ pad │
└──────────┴──────────┴───────┴──────────┴─────┘
```

- **TSC**: rdtsc timestamp (nanosecond precision)
- **src**: truncated lock address or identifier
- **ev**: event type (CLI, STI, LOCK_ACQ, LOCK_REL, IDLE_STI, etc.)
- **IF_after**: whether IF=1 or IF=0 after this event

At 100Hz timer + ~10 lock ops per tick, the buffer covers
~2 seconds of history — enough to capture the IF=0 transition
that never gets undone.

## Event Types

```rust
pub enum IfEvent {
    Cli          = 0,   // Explicit cli instruction
    Sti          = 1,   // Explicit sti instruction
    LockAcquire  = 2,   // SpinLock::lock() — saves RFLAGS + cli
    LockRelease  = 3,   // SpinLockGuard drop — restores RFLAGS
    IretqRing0   = 4,   // iretq returning to ring 0
    IretqRing3   = 5,   // iretq returning to ring 3
    SyscallEntry = 6,   // SYSCALL instruction (FMASK clears IF)
    Sysret       = 7,   // SYSRET returning to user
    IdleSti      = 8,   // idle loop's sti before hlt
    IdleCli      = 9,   // idle loop's cli after hlt
    SwitchSave   = 10,  // do_switch_thread: prev context saved
    SwitchLoad   = 11,  // do_switch_thread: next context loaded
}
```

## Instrumentation Points (Phase 3)

Currently instrumented:
- **SpinLock::lock()** → `LockAcquire` with lock address, `IF=0`
- **SpinLockGuard::drop()** → `LockRelease` with lock address, current IF state
- **idle()** → `IdleSti` before sti, `IdleCli` after cli

Future instrumentation (assembly, for Phase 3+):
- **trap.S iretq** → `IretqRing0` / `IretqRing3`
- **syscall_entry** → `SyscallEntry`
- **usermode.S sysret** → `Sysret`
- **do_switch_thread** → `SwitchSave` / `SwitchLoad`

## NMI Integration

When the NMI watchdog fires on a stuck CPU, it dumps the last 32
IF transitions:

```
========== NMI WATCHDOG: CPU 1 STUCK ==========
  RIP=0xffff800000234567  RSP=0xffff80000cafe000
  RFLAGS=0x0000000000000002  (IF=0, ring 0)
  ...
  if-trace: last 32 events for CPU 1 (of 847 total):
    [   0] tsc=1234567890 IDLE_STI   src=0x00000000 → IF=1
    [   1] tsc=1234567900  LOCK_ACQ  src=0x001a3c40 → IF=0   ← TIMERS.lock()
    [   2] tsc=1234568100  LOCK_REL  src=0x001a3c40 → IF=0   ← still IF=0 (nested)
    [   3] tsc=1234568200  LOCK_ACQ  src=0x001a4b80 → IF=0   ← SCHEDULER.lock()
    [   4] tsc=1234568500  LOCK_REL  src=0x001a4b80 → IF=0   ← still IF=0
    ...                                                        ← where's the IDLE_STI?
========== END NMI WATCHDOG ==========
```

This output immediately reveals: after the last lock release, IF was
still 0 (because `SavedInterruptStatus` restored IF=0 — it was saved
as IF=0 from the interrupt handler context).  The idle loop's `sti`
never executed because the CPU was stuck in `interval_work()` or
`switch()` or a spinloop — and the TSC gaps between events pinpoint
exactly where the CPU stalled.

## Performance

- **Disabled** (default at boot until `if_trace_enable()`): 1 atomic load per lock op (~1ns)
- **Enabled**: rdtsc + atomic store + 16-byte ring write per event (~15ns)
- Buffer: 4KB per CPU (256 × 16 bytes)

The overhead is negligible compared to the spinlock itself (~50-200ns).

## Files Changed

- `platform/x64/if_trace.rs` — new file: ring buffer, event enum, record(), dump()
- `platform/x64/mod.rs` — register module, export `if_trace_enable()`
- `platform/lib.rs` — export `if_trace_enable` from arch module
- `platform/spinlock.rs` — LockAcquire/LockRelease events in lock()/drop
- `platform/x64/idle.rs` — IdleSti/IdleCli events
- `platform/x64/interrupt.rs` — NMI handler calls if_trace::dump()
- `kernel/main.rs` — if_trace_enable() during boot

## Summary: The Diagnostic Stack

After Phases 1-3, a stuck CPU now produces:

1. **NMI Watchdog** (Phase 1): "CPU 1 is stuck" + RIP + RFLAGS + LAPIC state
2. **Lockdep** (Phase 2): "holding TIMERS (rank 10)"
3. **IF-Trace** (Phase 3): "last 32 IF transitions: ... stuck after LOCK_REL at TSC=X"

Together these three tools answer WHO (which CPU), WHAT (which lock),
WHERE (which instruction), and WHEN (TSC timestamp).  The only
question left is WHY — and the backtrace from the NMI handler
usually answers that too.
