# Phase 2: SMP Scheduler

**Goal:** Distribute runnable processes across CPUs. Each CPU runs its own
scheduling loop, pulling from per-CPU run queues with work stealing for
load balancing.

## Current Scheduler

The existing scheduler is a simple round-robin with a single global run queue
(`SCHEDULER` in `kernel/process/scheduler.rs`). On each timer tick (or
voluntary yield), the current process is pushed to the back of the queue and
the next is popped from the front. The queue is protected by a SpinLock.

## New Design: Per-CPU Run Queues

### Data Structures

```rust
pub struct PerCpuScheduler {
    /// Ready queue for this CPU.
    run_queue: SpinLock<VecDeque<Arc<Process>>>,
    /// Number of runnable processes (for load balancing decisions).
    nr_running: AtomicUsize,
}

/// Global array of per-CPU schedulers.
static CPU_SCHEDULERS: OnceCell<Vec<PerCpuScheduler>> = OnceCell::new();
```

### Scheduling Algorithm

Each CPU runs independently:

1. **Timer interrupt fires** → check if current process has exhausted its
   time slice (PREEMPT_PER_TICKS ticks, currently 3 = 30ms at 100 Hz).
2. **Preempt:** Push current process to back of this CPU's run queue.
3. **Pick next:** Pop front of this CPU's run queue. If empty, try to steal
   from another CPU.
4. **Context switch** to the chosen process.

### Work Stealing

When a CPU's run queue is empty:

```rust
fn steal_work(my_cpu: usize) -> Option<Arc<Process>> {
    let cpus = cpu_count();
    // Try each other CPU, starting from a random offset to avoid
    // thundering herd.
    let start = my_cpu.wrapping_add(1) % cpus;
    for i in 0..cpus {
        let target = (start + i) % cpus;
        if target == my_cpu { continue; }
        let target_sched = &CPU_SCHEDULERS[target];
        if target_sched.nr_running.load(Ordering::Relaxed) > 1 {
            // Steal one process from the back of the target's queue.
            if let Some(proc) = target_sched.run_queue.lock().pop_back() {
                target_sched.nr_running.fetch_sub(1, Ordering::Relaxed);
                return Some(proc);
            }
        }
    }
    None
}
```

### Process Placement

When a new process is created (fork/clone) or unblocked (signal, I/O ready):
- **Fork:** Place on the parent's CPU (cache locality).
- **Unblock:** Place on the CPU that last ran this process (stored in
  `Process.last_cpu`). If that CPU is heavily loaded, pick the least loaded.

### CPU Affinity (deferred)

`sched_setaffinity` / `sched_getaffinity` — not in initial implementation.
All processes can run on any CPU. Affinity support can be added later by
checking a bitmask before enqueuing.

## Inter-Processor Interrupt (IPI)

IPIs are needed when one CPU needs to wake another:

- **Reschedule IPI:** When process A on CPU 0 unblocks process B whose
  preferred CPU is CPU 1 (which is idle/running a lower-priority process).
  CPU 0 sends a reschedule IPI to CPU 1.
- **TLB shootdown IPI:** Covered in Phase 4.

### x86_64 IPI

Send via Local APIC ICR (Interrupt Command Register):

```rust
fn send_ipi(target_apic_id: u32, vector: u8) {
    // Write destination APIC ID to ICR high
    lapic_write(ICR_HIGH, (target_apic_id as u64) << 32);
    // Write vector + delivery mode to ICR low (triggers send)
    lapic_write(ICR_LOW, vector as u64);
}
```

Reserve vector 0xFE for reschedule IPI. The handler simply returns (the
timer interrupt handler will then check the run queue and reschedule).

### ARM64 IPI

Use GIC SGI (Software Generated Interrupt):

```rust
fn send_ipi(target_cpu: u32, irq: u32) {
    // GICv3: write to ICC_SGI1R_EL1
    let sgi = (target_cpu as u64) << 16 | irq as u64;
    write_sysreg!(icc_sgi1r_el1, sgi);
}
```

## Migration from Global to Per-CPU Queue

### Transition Strategy

1. Keep the global `SCHEDULER` working initially (Big Kernel Lock approach).
2. Add per-CPU queues alongside the global queue.
3. On each `schedule()` call, check per-CPU queue first, fall back to global.
4. Gradually move all enqueue/dequeue operations to per-CPU queues.
5. Remove global queue once per-CPU queues are stable.

### Wait Queue Changes

`WaitQueue::wake_all()` and `_wake_one()` currently push processes back to
the global queue. Change to push to the process's preferred CPU's queue:

```rust
pub fn wake_all(&self) {
    let mut queue = self.queue.lock();
    while let Some(process) = queue.pop_front() {
        let target_cpu = process.last_cpu();
        CPU_SCHEDULERS[target_cpu].enqueue(process);
        // Send reschedule IPI if target CPU is idle.
        if target_cpu != current_cpu_id() {
            send_reschedule_ipi(target_cpu);
        }
    }
}
```

## Idle Thread Changes

Each CPU now has its own idle thread (created in Phase 1). The idle thread
runs when the CPU's run queue is empty and work stealing finds nothing:

```rust
fn idle_loop() -> ! {
    loop {
        // Try to find work before halting.
        if let Some(next) = steal_work(current_cpu_id()) {
            switch_to(next);
        } else {
            enable_interrupts();
            halt();  // Wait for interrupt (timer or IPI)
        }
    }
}
```

## Reference Sources

- Linux sched(7) man page — scheduling concepts
- OSDev wiki — SMP scheduling

## Testing

- 4-CPU boot: 4 independent BusyBox shells (one per CPU) all responsive
- Process creation distributes across CPUs (check `/proc/[pid]/status` for
  CPU field, or add a sched_getcpu equivalent)
- Sleep/wake: process sleeping on CPU 0, woken by event on CPU 1, runs on
  appropriate CPU
- No deadlocks under load (fork bomb test with limit)
- Performance: `bench` suite shows wall-clock improvement with more CPUs
  for I/O-bound workloads
