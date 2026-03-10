# M6 Phase 2: SMP Scheduler

Kevlar now has a real SMP scheduler.  On a 4-vCPU guest each CPU runs its
own round-robin queue; when a queue empties, the CPU steals work from a
neighbour.  A new LAPIC timer fires at 100 Hz on each AP, triggering
`process::switch()` independently of the BSP's legacy PIT.

---

## The problem with a single run queue

Phase 1 left all three APs looping in `hlt`.  They were online — they just
had nothing to do.  The global `SCHEDULER` held one `VecDeque<PId>`.
Every `switch()` on every CPU locked the same spinlock and popped from the
same queue.  That's correct for a uniprocessor kernel, but it means:

* **No spatial locality**: a process that woke on CPU 2 might immediately
  migrate to CPU 0 on the next pick.
* **Contention**: every preemption across all CPUs serialises on the same
  lock.
* **APs idle forever**: without a per-CPU timer, APs never called
  `switch()` and never picked up work even when the queue was non-empty.

Phase 2 fixes all three issues.

---

## Per-CPU run queues

`Scheduler` now holds an array of eight independent queues:

```rust
pub struct Scheduler {
    run_queues: [SpinLock<VecDeque<PId>>; MAX_CPUS],
}
```

`enqueue` pushes to the calling CPU's slot; `pick_next` pops from it:

```rust
fn enqueue(&self, pid: PId) {
    let cpu = cpu_id() as usize % MAX_CPUS;
    self.run_queues[cpu].lock().push_back(pid);
}

fn pick_next(&self) -> Option<PId> {
    let cpu = cpu_id() as usize;
    let local = cpu % MAX_CPUS;

    // Local queue first.
    if let Some(pid) = self.run_queues[local].lock().pop_front() {
        return Some(pid);
    }

    // Work stealing: try other CPUs round-robin, stealing from the back.
    for i in 1..MAX_CPUS {
        let victim = (cpu + i) % MAX_CPUS;
        if let Some(pid) = self.run_queues[victim].lock().pop_back() {
            return Some(pid);
        }
    }
    None
}
```

The outer `SCHEDULER: SpinLock<Scheduler>` is still held during a full
`switch()` cycle (enqueue + pick_next), so the inner per-CPU locks are
never actually contested — they exist purely for interior mutability
through `&self`.  Stealing from the *back* of the victim's queue biases
towards recently-run processes (which are more likely to be cache-warm on
the victim CPU) while leaving its oldest, coldest work for locals.

### `cpu_id()`

Each CPU stores its index (0 = BSP, 1–N = APs in startup order) in a
`cpu_local!` variable:

```rust
cpu_local! {
    pub static ref CPU_ID: u32 = 0;
}

pub fn cpu_id() -> u32 {
    *CPU_ID.get()
}
```

The BSP's `CPU_ID` defaults to 0.  Before sending each SIPI, the BSP
writes the next index to `AP_CPU_ID: AtomicU32`; the AP reads it in
`ap_rust_entry` and calls `CPU_ID.set(ap_cpu_id)` after `cpu_local::init`
establishes the GSBASE.

---

## LAPIC timer for AP preemption

The BSP has used the PIT at 100 Hz since M1.  APs have no connection to
the PIT (it's routed through the I/O APIC as IRQ 0, which delivers only to
the BSP).  Each AP needs its own periodic interrupt.

### Calibration (BSP, once)

The LAPIC timer counts down from an initial value at the local bus clock
rate.  After TSC calibration, the BSP measures how many LAPIC ticks happen
in 10 ms and stores the result:

```rust
pub unsafe fn lapic_timer_calibrate() {
    lapic_write(LAPIC_DIV_CONF_OFF, 0xB);          // divide by 1
    lapic_write(LAPIC_LVT_TIMER_OFF,
        LAPIC_TIMER_MASKED | LAPIC_PREEMPT_VECTOR as u32);
    lapic_write(LAPIC_INIT_COUNT_OFF, u32::MAX);

    let start = tsc::nanoseconds_since_boot();
    while tsc::nanoseconds_since_boot() - start < 10_000_000 {}

    let remaining = lapic_read(LAPIC_CURR_COUNT_OFF);
    lapic_write(LAPIC_INIT_COUNT_OFF, 0); // stop
    LAPIC_TICKS_PER_10MS.store(u32::MAX.wrapping_sub(remaining), Ordering::Relaxed);
}
```

### Per-CPU timer start

Every AP calls `lapic_timer_init()` after process state is ready:

```rust
pub unsafe fn lapic_timer_init() {
    let ticks = LAPIC_TICKS_PER_10MS.load(Ordering::Relaxed);
    lapic_write(LAPIC_DIV_CONF_OFF, 0xB);
    lapic_write(LAPIC_LVT_TIMER_OFF,
        LAPIC_TIMER_PERIODIC | LAPIC_PREEMPT_VECTOR as u32);
    lapic_write(LAPIC_INIT_COUNT_OFF, ticks);
}
```

`LAPIC_PREEMPT_VECTOR = 0x40` (64) fires on the AP's own local APIC.
The interrupt handler catches it before the generic IRQ dispatcher:

```rust
match vec {
    LAPIC_PREEMPT_VECTOR => {
        ack_interrupt();
        handler().handle_ap_preempt();
    }
    _ if vec >= VECTOR_IRQ_BASE => { /* IRQ 0–15 … */ }
    // …
}
```

`handle_ap_preempt` calls `process::switch()`.

---

## AP kernel entry and the `KERNEL_READY` gate

An AP completes platform setup well before the BSP finishes initialising
the VFS, device drivers, and the process subsystem.  Calling
`process::init_ap()` too early panics because `INITIAL_ROOT_FS` — used
even by the idle thread constructor — is not yet set.

The fix is a single atomic flag:

```rust
static KERNEL_READY: AtomicBool = AtomicBool::new(false);
```

The BSP sets it immediately after `process::init()`:

```rust
process::init();
KERNEL_READY.store(true, Ordering::Release);
```

Each AP spins on it in `ap_kernel_entry`:

```rust
pub fn ap_kernel_entry() -> ! {
    while !KERNEL_READY.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }
    process::init_ap();          // idle thread + CURRENT
    start_ap_preemption_timer(); // LAPIC timer (safe now that CURRENT is valid)
    switch();
    idle_thread()
}
```

Starting the LAPIC timer *after* `process::init_ap()` is critical: the
timer handler calls `process::switch()`, which dereferences `CURRENT`.  If
the timer fires before `CURRENT` is set the AP panics on an uninitialised
`Lazy`.

---

## Results

```
acpi: found 4 Local APIC(s)
CPU (LAPIC 1, cpu_id=1) online
CPU (LAPIC 2, cpu_id=2) online
CPU (LAPIC 3, cpu_id=3) online
smp: 3 AP(s) online, total 4 CPU(s)
Booting Kevlar...
```

All 31 existing tests pass under `-smp 4` (TCG and KVM).  Processes
enqueued by the init script are picked up by whichever CPU gets there
first; work stealing ensures APs don't idle while the BSP queue is
non-empty.

---

## What's next

Each AP now participates in scheduling, but the implementation is still
coarse-grained: all preemption decisions share a single global spinlock.
M6 Phase 3 will tackle the next prerequisite for Wine: `pthread_create`
end-to-end, which requires `futex(FUTEX_WAKE)` to wake a thread sleeping
on a specific CPU.

| Phase | Description | Status |
|-------|-------------|--------|
| M6 Phase 1 | SMP boot (INIT-SIPI-SIPI, trampoline, MADT) | ✅ Done |
| M6 Phase 2 | Per-CPU run queues + LAPIC timer preemption | ✅ Done |
| M6 Phase 3 | Futex wake-on-CPU, pthread_create end-to-end | 🔄 Next |
