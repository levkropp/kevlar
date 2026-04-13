# Blog 154: Milestone T — Dynamic Analysis Tooling & NMI Watchdog

**Date:** 2026-04-12

## The Problem: "CPU Goes Silent"

The XFCE desktop test (Blog 153) exposed a class of bug that's nearly
impossible to diagnose with static analysis: the LAPIC timer fires
exactly **once** per CPU then stops forever.  Both CPUs show heartbeat
counter = 1, then zero increments for 300 seconds.

The evidence:
- LAPIC hardware is correctly configured (LVT=0x20040, periodic, unmasked)
- ack_interrupt() (EOI) is called before any work
- The initial count register is non-zero, current count is counting down
- After the first handler call does a context switch via `switch()`,
  the LAPIC never fires again on either CPU

We traced every code path — idle loop, forked_child_entry, do_switch_thread,
interrupt return — and they all look correct.  The IF flag should be
restored via iretq or sti.  But something in the SMP context switch path
permanently disables interrupts on both CPUs, and we have no visibility
into *where* or *why*.

This is the exact class of bug that Linux's hardlockup detector was built
to catch — except Linux took 15 years to get there.  We're building it
now, at kernel line ~50K, when it matters most.

## Milestone T: Dynamic Analysis Tooling

We designed a 7-phase milestone dedicated to diagnostic infrastructure:

| Phase | Tool | What it catches |
|-------|------|----------------|
| 1 | **NMI Watchdog** | CPUs stuck with IF=0 (the LAPIC bug) |
| 2 | **Lock Dependency Validator** | Deadlocks (TIMERS→SCHEDULER class) |
| 3 | **Interrupt State Tracker** | IF transition history (CLI/STI/IRETQ) |
| 4 | **Guard Pages** | Kernel stack overflows |
| 5 | **Enhanced Flight Recorder** | Cross-CPU event correlation |
| 6 | **Preemption Safety Checker** | Per-CPU data races |
| 7 | **Documentation & Integration** | Make tools discoverable |

The philosophy: build the tools Linux wishes it had from day one, while
the kernel is small enough to instrument thoroughly.  Every phase is
designed to make the *next* bug faster to find.

## Phase 1: NMI Watchdog (Hard Lockup Detector)

### Design

The NMI (Non-Maskable Interrupt) is the only interrupt that fires even
when IF=0.  This is the fundamental insight: a CPU stuck in a spinloop
with interrupts disabled will ignore the LAPIC timer, PIT, and all
other maskable interrupts — but it *cannot* ignore an NMI.

**Architecture:**

```
  LAPIC timer fires (100Hz)
       │
       ▼
  lapic_hb_inc()        ← increment per-CPU heartbeat counter
  ack_interrupt()       ← send EOI
  handle_ap_preempt()   ← timer processing + context switch
       │
       ▼
  watchdog_check()      ← called from handle_timer_irq()
       │                   every ~400 ticks (~4 seconds)
       │
  ┌────┴────┐
  │ Compare │  LAPIC_HB[cpu] vs LAST_HB_SNAPSHOT[cpu]
  │ stalled?│  for each online CPU
  └────┬────┘
       │ yes
       ▼
  send_nmi_ipi(apic_id) ← LAPIC ICR, delivery mode = NMI
       │
       ▼
  NMI handler (vector 2)
  ┌─────────────────────────────────────┐
  │ Lock-free state dump:               │
  │ • RIP, RSP, RFLAGS (IF bit!)       │
  │ • preempt_count, need_resched       │
  │ • LAPIC timer registers (LVT, etc) │
  │ • RBP-chain backtrace (16 frames)  │
  │ • Flight recorder event            │
  └─────────────────────────────────────┘
```

### Key Constraint: Lock-Free NMI Handler

The NMI can interrupt **any** code, including code that holds a
spinlock.  If the NMI handler tried to acquire the same lock, instant
deadlock.  Therefore:

- All NMI output uses `log::warn!()` which goes through a `spin::Mutex`
  (different from our `SpinLock` — it's only used by the logger)
- If the logger lock is poisoned, we fall through silently rather than
  panic (double-fault from NMI handler is unrecoverable)
- LAPIC register reads are raw MMIO — no lock needed
- CpuLocalHead fields are read directly from GSBASE — no lock needed
- Backtrace walks raw RBP pointers — no lock needed

### Implementation

**NMI IPI sending** (`platform/x64/apic.rs`):
```rust
pub unsafe fn send_nmi_ipi(apic_id: u8) {
    const ICR_NMI: u32 = 0x400; // delivery mode = NMI
    wait_icr_idle();
    lapic_write(ICR_HIGH_OFF, (apic_id as u32) << 24);
    lapic_write(ICR_LOW_OFF, ICR_NMI);
}
```

**Heartbeat counter** — incremented at the very top of the LAPIC handler,
before `ack_interrupt()`, before any lock:
```rust
LAPIC_PREEMPT_VECTOR => {
    super::apic::lapic_hb_inc();  // per-CPU atomic increment
    ack_interrupt();
    // ... rest of handler
}
```

**APIC ID registration** — each CPU registers its APIC ID during boot
so the watchdog can target NMI IPIs by CPU index:
```rust
// BSP (cpu 0):
register_cpu_apic_id(0);
// Each AP:
register_cpu_apic_id(cpu_id());
```

### LAPIC Timer Diagnostic Mode

As a bonus, we added a `DIAG_SKIP_SWITCH` flag in `kernel/timer.rs`:

```rust
pub const DIAG_SKIP_SWITCH: bool = false;
```

When set to `true`, the timer handler runs normally (decrement timers,
wake poll waiters, update clocks) but **skips the context switch**.
If the LAPIC heartbeat keeps incrementing with this flag set, the bug
is definitively in `switch()`'s interaction with the interrupt return
path.  This is a binary search over the handler's code, isolating the
fault in one boot cycle.

### Boot Test Results

SMP=2 boot with watchdog enabled:

```
watchdog: NMI hard lockup detector enabled
LAPIC-DIAG cpu=0 LVT=0x20040 INIT=10009175 CURR=9154687 DIV=0xb HB=[0, 0]
LAPIC-DIAG cpu=0 LVT=0x20040 INIT=10009175 CURR=8613152 DIV=0xb HB=[0, 0]
LAPIC-DIAG cpu=0 LVT=0x20040 INIT=10009175 CURR=8132493 DIV=0xb HB=[0, 0]
LAPIC-DIAG cpu=0 LVT=0x20040 INIT=10009175 CURR=6294342 DIV=0xb HB=[499, 0]
LAPIC-DIAG cpu=0 LVT=0x20040 INIT=10009175 CURR=9859744 DIV=0xb HB=[1005, 0]
LAPIC-DIAG cpu=0 LVT=0x20040 INIT=10009175 CURR=6203060 DIV=0xb HB=[1505, 0]
```

- First 3 lines: pre-`sti` idle loop, heartbeat = 0 (expected)
- After `sti`: HB[0] climbs ~500/check = 100Hz, confirming timer works
- HB[1] = 0: AP is running user processes, not hitting idle's diagnostic
- No false-alarm NMIs during normal boot

When run against the XFCE workload that triggers the LAPIC stall, the
watchdog will catch the stuck CPU within 4 seconds and dump the exact
RIP, RFLAGS, and backtrace — telling us whether the CPU is in a
spinloop, in `do_switch_thread`, in `interval_work`, or somewhere
unexpected.

## What's Next: Phase 2 (Lock Dependency Validator)

Phase 2 will add compile-time and runtime lock ordering enforcement.
Every lock gets a rank; acquiring a lower-rank lock while holding a
higher-rank lock panics immediately with the full lock chain.  This
would have caught the TIMERS→SCHEDULER and WaitQueue→SCHEDULER
deadlocks instantly instead of manifesting as mysterious timer stalls.

## Files Changed

- `platform/x64/apic.rs` — LAPIC_HB counters, watchdog_check(), send_nmi_ipi(), APIC ID table, lapic_timer_diag_log()
- `platform/x64/interrupt.rs` — NMI handler state dump, heartbeat increment in LAPIC handler
- `platform/x64/mod.rs` — public API wrappers (register_cpu_apic_id, watchdog_enable, watchdog_check)
- `platform/lib.rs` — arch module re-exports
- `platform/flight_recorder.rs` — NMI_WATCHDOG event kind (12)
- `kernel/timer.rs` — DIAG_SKIP_SWITCH flag, watchdog_check() call in handle_timer_irq
- `kernel/main.rs` — APIC ID registration, watchdog enable, periodic LAPIC-DIAG output
