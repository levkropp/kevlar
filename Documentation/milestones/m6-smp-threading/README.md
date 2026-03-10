# Milestone 6: SMP & Threading

**Goal:** Boot on multiple CPU cores and support multi-threaded userspace
programs via clone(CLONE_VM). Run a pthreads program linked against musl
or glibc.

**Current state:** Kevlar runs on a single CPU core. The scheduler is
single-queue. SpinLocks use cli/sti + atomics (SMP-ready primitives, but
never tested with actual concurrent cores). clone() exists but returns
ENOSYS for CLONE_VM/CLONE_THREAD. No TLB shootdowns, no per-CPU data, no
inter-processor interrupts.

## Phases

| Phase | Name | Key Changes | Prerequisite |
|-------|------|------------|--------------|
| [1](phase1-smp-boot.md) | SMP Boot | AP startup, per-CPU state, per-CPU idle threads | None |
| [2](phase2-smp-scheduler.md) | SMP Scheduler | Per-CPU run queues, load balancing, IPI | Phase 1 |
| [3](phase3-threading.md) | Threading Primitives | clone(CLONE_VM), TLS, robust futex | Phase 2 |
| [4](phase4-thread-safety.md) | Thread Safety | TLB shootdowns, signal-to-thread, audit | Phase 3 |
| [5](phase5-integration.md) | Integration Testing | pthreads test, glibc hello world | All above |

Phases are strictly sequential — each builds on the previous.

## Why SMP Before Threading

Threading without SMP is just time-sharing on one core — it works but isn't
useful for real parallelism. SMP without threading still benefits the kernel
(interrupt handling on different cores, running independent processes in
parallel) and is a prerequisite for correct threading.

The natural order:
1. SMP boot → multiple cores running kernel code
2. SMP scheduler → processes distributed across cores
3. Threading → shared address space within a process
4. Safety audit → ensure everything is correct under concurrency

## Architectural Impact

SMP is the most invasive change since ARM64 support. It touches:

- **Platform layer:** AP startup, per-CPU GDT/TSS/IDT (x86), per-CPU stacks,
  per-CPU current_process pointer, LAPIC timer, IPI, TLB shootdown
- **Scheduler:** Per-CPU run queues, load balancing, migration, CPU affinity
- **Memory management:** TLB shootdowns when page tables change, per-CPU page
  caches, atomic reference counts on shared page tables
- **Signals:** Delivery to specific threads (tgkill), signal masks per-thread
- **Process model:** Thread group (TGID vs PID), shared fd table, shared
  signal handlers, per-thread signal mask and stack

## Design Decisions

1. **Per-CPU data via GS segment (x86_64) / TPIDR_EL1 (ARM64).** Fast,
   no locks needed. Store: current_process, CPU ID, kernel stack pointer,
   idle thread reference.

2. **Per-CPU run queues with work stealing.** Each CPU has its own ready
   queue. When a CPU's queue is empty, it steals from the busiest CPU.
   Simple and well-understood design (used by Linux CFS).

3. **Big Kernel Lock (BKL) as a transitional step.** Before the full SMP
   scheduler is ready, a BKL lets us boot multiple cores safely by
   serializing kernel entry. Remove the BKL incrementally as fine-grained
   locking is verified. (Optional — skip if confident in existing locking.)

4. **Thread model: 1:1 kernel threads.** Each userspace thread maps to one
   kernel thread (one Process struct). Threads in the same group share
   address space and fd table via Arc. This matches Linux's model.

5. **TLB shootdown via IPI.** When a process modifies its page tables (mmap,
   munmap, mprotect), send IPI to all CPUs running threads of that process
   to flush their TLBs. Use a simple broadcast initially, refine to targeted
   IPIs later.

## Reference Sources

- FreeBSD SMP: `sys/x86/x86/mp_x86.c` (BSD-2-Clause) — AP startup
- FreeBSD scheduler: `sys/kern/sched_ule.c` (BSD-2-Clause) — ULE scheduler
- OSDev wiki: SMP, APIC, Inter-Processor Interrupts
- Intel SDM Volume 3: Chapter 8 (Multiple-Processor Management)
- ARM Architecture Reference Manual: PSCI, spin-table boot protocol

## Success Criteria

- Kernel boots on 4 CPUs (QEMU `-smp 4`)
- `cat /proc/cpuinfo` shows 4 processors
- Independent processes run on different cores simultaneously
- A pthreads program (static musl) creates threads, runs parallel work,
  joins correctly
- No deadlocks, no data corruption under stress
- All existing tests pass with `-smp 4`
- ARM64: same tests on QEMU virt with 4 cores
