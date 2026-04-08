# Blog 149: SMP Deadlock Fixed — Interrupt Gate IF=0 Propagation

**Date:** 2026-04-07

## The Bug

Blog 147 documented an undiagnosed SMP deadlock: when two at-spi-bus-launcher
processes crashed simultaneously on `-smp 2`, both CPUs went completely silent.
No serial output, no timer heartbeats, no panic message. QEMU showed ~200% CPU
(both vCPUs spinning). The workaround was running XFCE tests on `-smp 1`.

## Root Cause: Interrupt Gate Propagates IF=0 Through Entire Exit Path

The BREAKPOINT handler (INT3, used by `__stack_chk_fail`) is configured as an
**interrupt gate** in the IDT (type 0xEE). Interrupt gates automatically clear
the CPU's interrupt flag (IF=0) on entry. This is the correct x86 behavior for
safely entering an interrupt handler.

The problem: the exit path — `exit_by_signal()` -> `Process::exit()` ->
`switch()` — ran with IF=0 for its **entire duration**. Here's why:

`SpinLock::lock()` saves RFLAGS (via `pushfq`), disables interrupts (`cli`),
acquires the lock, and restores RFLAGS on drop (`popfq`). This is correct
when called from normal code: it saves IF=1, disables to IF=0, and restores
IF=1 when the lock is released.

But when called from inside an interrupt gate handler, the saved RFLAGS already
has IF=0. Every lock acquire/release cycle becomes: save IF=0, `cli` (no-op),
acquire, release, restore IF=0. **Interrupts are never re-enabled.**

```
Normal code path:           Interrupt gate path:
  IF=1                        IF=0 (cleared by hardware)
  SpinLock::lock()            SpinLock::lock()
    save IF=1, cli              save IF=0, cli (no-op)
    ... critical section ...    ... critical section ...
  SpinLock::drop()            SpinLock::drop()
    restore IF=1                restore IF=0    <-- stays disabled!
  IF=1                        IF=0             <-- forever
```

When two processes crash simultaneously on two CPUs, both run their exit paths
with interrupts permanently disabled. Any spinlock contention becomes unbounded
-- no timer IRQ can fire to preempt, no heartbeats, no progress signals. The
CPUs spin on whichever lock they're contending for, and neither can ever yield.

This isn't a classic AB-BA lock ordering violation. The lock ordering in the
exit path is consistent (WaitQueue -> SCHEDULER -> PROCESSES). The bug is that
the IF=0 state turns any momentary contention into a permanent hang.

## Fix 1: Re-enable Interrupts Before Exit Path

Added `enable_interrupts()` at the top of `exit_by_signal()`, before any lock
is touched:

```rust
pub fn exit_by_signal(signal: Signal) -> ! {
    // Re-enable interrupts.  Fault handlers may enter via interrupt gates
    // (which clear IF).  Running the entire exit path with IF=0 means
    // SpinLock save/restore cycles never re-enable interrupts.
    kevlar_platform::arch::enable_interrupts();
    ...
}
```

This matches Linux's `cond_local_irq_enable()` which is called early in every
trap handler. The new `enable_interrupts()` function is implemented for both
architectures: `sti` on x86_64, `msr daifclr, #2` on ARM64.

With IF=1 restored, SpinLock save/restore cycles work correctly again. Lock
contention is bounded because the timer IRQ can fire between lock regions.

## Fix 2: Break SCHEDULER -> PROCESSES Lock Nesting in switch()

While auditing lock ordering, I found that `switch()` held SCHEDULER while
acquiring PROCESSES (to resolve the picked PID to an `Arc<Process>`). This was
the only nested global lock pair in the kernel:

```rust
// Before: SCHEDULER held while acquiring PROCESSES
let next = {
    let scheduler = SCHEDULER.lock();       // Phase 1+2 combined
    scheduler.enqueue(prev_pid);
    match scheduler.pick_next() {
        Some(pid) => PROCESSES.lock().get(&pid), // nested!
        None => idle_thread(),
    }
};
```

Split into two non-overlapping phases:

```rust
// After: locks never overlap
let next_pid = {
    let scheduler = SCHEDULER.lock();       // Phase 1: SCHEDULER only
    scheduler.enqueue(prev_pid);
    scheduler.pick_next()
};                                          // SCHEDULER released

let next = match next_pid {                 // Phase 2: PROCESSES only
    Some(pid) => PROCESSES.lock().get(&pid),
    None => idle_thread(),
};
```

The window between releasing SCHEDULER and acquiring PROCESSES is safe: if
`exit_group()` removes the chosen PID in that window, the PROCESSES lookup
returns None and we fall back to the idle thread. This fallback already existed
for the same race condition.

## Verification

XFCE test now runs on `-smp 2`. Both at-spi-bus-launcher crashes complete
cleanly with continued serial output (previously both CPUs went silent):

```
USER FAULT: BREAKPOINT pid=24 ip=0xa1035eed0
PID 24 (/usr/libexec/at-spi-bus-launcher) killed by signal 11
PID 28 (/usr/bin/dbus-daemon ...) killed by signal 9
USER FAULT: BREAKPOINT pid=38 ip=0xa1035eed0
PID 38 (/usr/libexec/at-spi-bus-launcher) killed by signal 11
```

SMP regression tests: 14/14 threading tests PASS, 15/15 mini_systemd PASS,
all on `-smp 4`.

## Files Changed

- `platform/x64/mod.rs`: Added `enable_interrupts()` (`sti`)
- `platform/arm64/mod.rs`: Added `enable_interrupts()` (`msr daifclr, #2`)
- `platform/lib.rs`: Export `enable_interrupts` for both architectures
- `kernel/process/process.rs`: Call `enable_interrupts()` at top of `exit_by_signal()`
- `kernel/process/switch.rs`: Split SCHEDULER/PROCESSES into non-overlapping phases
- `Makefile`: Restored `-smp 2` for `test-xfce` target

## Lesson

Interrupt gates are a trap. The IF=0 state is invisible — it doesn't cause any
immediate error, and the system works perfectly on single CPU or when processes
don't exit concurrently. The SpinLock implementation is correct in isolation;
the bug is that nobody re-enables interrupts between the hardware clearing IF
and the kernel entering long-running code paths.

Linux solves this with `cond_local_irq_enable()` called at the top of every
exception handler. It's one of those small details that's easy to overlook when
building a kernel from scratch, but critical for SMP correctness.
