# Phase 4: Thread Safety Audit & TLB Shootdowns

**Goal:** Ensure the kernel is correct under true concurrent execution.
Implement TLB shootdowns, fix signal delivery for thread groups, and audit
shared data structures.

## TLB Shootdowns

When a process modifies its page tables (mmap, munmap, mprotect, execve),
other CPUs running threads of the same process may have stale TLB entries.
We must flush their TLBs.

### When to Shootdown

- `munmap()` — pages unmapped, must invalidate
- `mprotect()` — permissions changed, must invalidate
- `execve()` — entire address space replaced
- Page fault handler — COW page promoted (shared → private copy)

### Implementation

```rust
fn tlb_shootdown(process: &Process, start: VAddr, end: VAddr) {
    let my_cpu = current_cpu_id();

    // Find all CPUs running threads of this process.
    for cpu in 0..cpu_count() {
        if cpu == my_cpu { continue; }
        let running = CPU_SCHEDULERS[cpu].current_process();
        if running.tgid == process.tgid {
            // Send TLB flush IPI to that CPU.
            send_tlb_flush_ipi(cpu, start, end);
        }
    }

    // Flush our own TLB.
    flush_tlb_range(start, end);

    // Wait for all target CPUs to acknowledge the flush.
    wait_for_flush_ack();
}
```

### IPI Protocol

Reserve vector 0xFD for TLB shootdown IPI (x86_64):

1. Sender writes flush range to a shared `TlbFlushRequest` struct.
2. Sender sends IPI to target CPUs, sets `pending_ack` counter.
3. Target CPU's IPI handler reads the flush request, executes `invlpg` for
   each page (or `mov cr3, cr3` for full flush), decrements `pending_ack`.
4. Sender spins on `pending_ack` reaching 0.

For small ranges (< 16 pages): individual `invlpg` instructions.
For large ranges: full TLB flush (`mov cr3, cr3` on x86_64).

ARM64: `tlbi vale1is` (TLB Invalidate by VA, Last-level, EL1, Inner Shareable)
broadcasts automatically in the inner shareable domain. Explicit IPI may not
be needed if all CPUs share the same inner shareable domain (they do on QEMU
virt). Verify with `dsb ish` barrier.

## Signal Delivery to Thread Groups

### Current Model

Signals are delivered to a specific process (PID). With threads, process-directed
signals (kill(pid)) should go to any thread in the thread group.

### New Model

- **Process-directed signals** (kill, SIGCHLD from child): delivered to any
  thread in the group that hasn't blocked the signal. Prefer the main thread.
- **Thread-directed signals** (tgkill, synchronous faults like SIGSEGV):
  delivered to the specific thread.
- **Signal handlers** are shared (CLONE_SIGHAND). Signal masks are per-thread.

```rust
fn deliver_signal_to_group(tgid: PId, signal: Signal) {
    let group = find_thread_group(tgid);
    // Find a thread that hasn't blocked this signal.
    for thread in group.threads() {
        if !thread.sigset_load().is_blocked(signal) {
            thread.send_signal(signal);
            return;
        }
    }
    // All threads blocked it — queue on the group leader.
    group.leader().send_signal(signal);
}
```

### exit_group

`exit_group()` must terminate all threads in the group:

```rust
fn sys_exit_group(status: i32) {
    let group = current_process().thread_group();
    for thread in group.threads() {
        if thread.pid() != current_process().pid() {
            thread.send_signal(SIGKILL);
        }
    }
    Process::exit(status);
}
```

## Data Structure Audit

### Already SMP-Safe (SpinLock protected)

- `OpenedFileTable` — protected by SpinLock, shared via Arc for CLONE_FILES
- `VirtualMemory` / VMAs — protected by SpinLock
- `SignalDelivery` — protected by SpinLock
- Process list (`PROCESSES`) — protected by SpinLock
- Network stack — SpinLock protected
- tmpfs/initramfs inodes — SpinLock protected

### Needs Review

| Structure | Risk | Fix |
|-----------|------|-----|
| `current_process()` | Was global, now per-CPU | Use PerCpu struct |
| `MONOTONIC_TICKS` | AtomicUsize, fine | Verify Ordering |
| `SCHEDULER` | Global queue → per-CPU | Phase 2 |
| `POLL_WAIT_QUEUE` | Global wait queue | May bottleneck under SMP; consider per-process |
| Page allocator | Global lock | May bottleneck; per-CPU page cache helps |
| `JOIN_WAIT_QUEUE` | Global | Fine for now, optimize if profiling shows contention |
| Pipe buffers | Per-pipe SpinLock | Fine |

### Lock Ordering

Document and enforce a lock ordering to prevent deadlocks:

```
1. Process list (PROCESSES)
2. Per-process locks (in order):
   a. VirtualMemory
   b. OpenedFileTable
   c. SignalDelivery
3. Filesystem locks (per-inode)
4. Network locks (per-socket)
5. Wait queue locks
6. Scheduler locks (per-CPU)
```

Never acquire a higher-numbered lock while holding a lower-numbered one.

## Reference Sources

- Intel SDM Volume 3 — TLB management
- Linux man pages: tgkill(2), signal(7) — thread signal semantics

## Testing

- mprotect on one thread, verify other threads see the change immediately
- munmap shared page, verify other threads fault correctly
- Send SIGTERM to process, verify one thread receives it
- tgkill specific thread, verify only that thread gets the signal
- exit_group from one thread, verify all threads terminate
- Stress test: 4 threads fork-bombing on 4 CPUs, no deadlock or panic
- Existing mini_systemd and bench tests pass with `-smp 4`
