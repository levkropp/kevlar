# M6 Phase 3.5: SMP Debug Tooling and the WaitQueue Race

After Phase 3 landed, 12/12 threading tests passed on a single vCPU.  Under
`-smp 4` they hung — specifically at test 6, the mutex test, which would block
forever waiting for a `pthread_join` that never returned.

A hanging mutex test on SMP almost always means a thread is lost: no longer in
any scheduler queue or wait queue, so nobody will ever wake it.  Diagnosing
*why* required better crash-time visibility than we had, so we shipped four
debug tooling improvements before touching any threading code.

---

## Improvement 1: kernel register dump on fault

Before: a kernel page fault or general protection fault would print a one-line
panic message and halt.

After: the interrupt handler dumps the full register set, the fault address
(CR2), and the kernel stack contents at RSP before calling `panic!`:

```
kernel page fault — register dump:
  RIP=ffffffff80123456  RSP=ffffffff8012a000  RBP=ffffffff8012a0f0
  RAX=0000000000000000  RBX=ffff800040001234  RCX=0000000000000003  RDX=0000000000000000
  RSI=0000000000000001  RDI=ffff800040001234  R8 =0000000000000000  R9 =0000000000000000
  R10=0000000000000000  R11=0000000000000000  R12=0000000000000001  R13=0000000000000000
  R14=0000000000000000  R15=0000000000000000
  CS=0x8 (ring 0)  SS=0x10  RFLAGS=0x00000046  ERR=0x2
  CR2 (fault vaddr) = 0000000000000000
  kernel stack at RSP (ffffffff8012a000):
    [rsp+0x00] = ffffffff80123456
    [rsp+0x08] = 0000000000000000
    [rsp+0x10] = ffff800040001234
    …
```

The stack dump is particularly useful for identifying null function-pointer
crashes: if RIP is 0, the return address chain in the stack usually points to
the actual caller.

The same treatment was applied to GPF, invalid opcode, and the other
synchronous exceptions — anything that previously just panicked with a bare
`{:?}` of the packed `InterruptFrame`.

---

## Improvement 2: unconditional page poison

Before: freed pages were only poisoned in debug builds.  Release builds returned
clean (or zero) memory, hiding use-after-free bugs until they caused data
corruption far from the original site.

After: every freed page is written with `0xa5` in all build profiles, including
`profile-performance` and `profile-ludicrous`.  The cost is roughly one cache
miss per freed page — negligible for kernel workloads.

The immediate effect: a use-after-free that previously looked like "wrong but
plausible data" now produces a crash with RIP or a pointer containing
`0xa5a5a5a5a5a5a5a5`.  Much faster to diagnose.

---

## Improvement 3: per-CPU lock-free flight recorder

The most useful addition.  The flight recorder is a fixed-size circular buffer
of recent events per CPU, written at interrupt speed and dumped by the panic
handler after all other CPUs are halted.

### Design

```
platform/flight_recorder.rs:
  MAX_CPUS  = 8
  RING_SIZE = 64 entries per CPU

Entry layout (32 bytes = 4 × u64):
  [0] tsc         : u64   — raw TSC timestamp
  [1] kind:u8 | cpu:u8 | _pad:u16 | data0:u32  — packed descriptor
  [2] data1       : u64
  [3] data2       : u64
```

The `static mut RINGS` array is indexed `[cpu][entry][word]`.  Only CPU `n`
writes to `RINGS[n]` — so no synchronisation is needed on the write path.  The
index counter `IDX[n]` uses a single relaxed atomic increment.  The dump path
is safe because all peer CPUs are halted before `dump()` runs.

```rust
#[inline(always)]
pub fn record(kind: u8, data0: u32, data1: u64, data2: u64) {
    let cpu = crate::arch::cpu_id() as usize % MAX_CPUS;
    let raw_idx = IDX[cpu].fetch_add(1, Ordering::Relaxed);
    let idx = raw_idx % RING_SIZE;
    let tsc = crate::arch::read_clock_counter();
    unsafe {
        let slot = &mut RINGS[cpu][idx];
        slot[0] = tsc;
        slot[1] = ((kind as u64) << 56) | ((cpu as u64) << 48) | (data0 as u64);
        slot[2] = data1;
        slot[3] = data2;
    }
}
```

`dump()` collects all non-zero entries from all CPUs, insertion-sorts them by
TSC (≤512 entries, O(n²) is fine in the panic path), and prints a
cross-CPU timeline:

```
[FLIGHT RECORDER — last 64 events per CPU, sorted by TSC]
  (base TSC=0x1234abcd, showing 47 events)
  +       0 ticks  CPU=0  CTX_SWITCH   CPU=0 CTX_SWITCH  from_pid=1 to_pid=2
  +     412 ticks  CPU=1  PREEMPT      CPU=1 PREEMPT     pid=3
  +     430 ticks  CPU=1  CTX_SWITCH   CPU=1 CTX_SWITCH  from_pid=3 to_pid=4
  +    1024 ticks  CPU=0  SYSCALL_IN   CPU=0 SYSCALL_IN  nr=202 arg0=0x7f00
  …
```

### Integration points

| Location | Event | Data |
|----------|-------|------|
| `kernel/process/switch.rs` | `CTX_SWITCH` | from_pid, to_pid |
| `platform/x64/apic.rs` (`tlb_shootdown`) | `TLB_SEND` | target CPU mask, vaddr |
| `platform/x64/apic.rs` (`tlb_remote_full_flush`) | `TLB_SEND` | target CPU mask, 0 |
| `platform/x64/interrupt.rs` (TLB IPI handler) | `TLB_RECV` | vaddr invalidated |
| `platform/x64/interrupt.rs` (LAPIC preempt) | `PREEMPT` | CPU id |
| `platform/x64/idle.rs` | `IDLE_ENTER` / `IDLE_EXIT` | — |

The recorder costs nothing at runtime on the non-panicking path — no locks, no
branches, no conditional compilation.

---

## Improvement 4: serial-based crash dump

The original crash dump mechanism used `boot2dump` — a mini bootloader embedded
in the binary that, on panic, wrote the kernel log to an ext4 file on a
virtio-blk device and then rebooted.  This never worked in our QEMU test setup
(no virtio-blk) and added ~800 KB to the binary.

Replacement: the panic handler base64-encodes the `KernelDump` struct (magic +
log length + 4 KiB of log) and emits it over the existing serial debug printer,
framed by sentinel lines:

```
===KEVLAR_CRASH_DUMP_BEGIN===
AAECAw...base64...
===KEVLAR_CRASH_DUMP_END===
```

The encoder runs inline in the panic handler with no allocation — just a `const`
alphabet slice and a loop over 3-byte groups.

`run-qemu.py` gains a `--save-dump FILE` flag.  When set, it spawns a thread
that intercepts QEMU's stdout, scans for the sentinels, base64-decodes on the
fly, and writes the decoded bytes to `FILE`.  `make run` now passes
`--save-dump kevlar.dump` automatically, so crash dumps land in the working
directory without any user action.

---

## The bug: WaitQueue lost-thread race

With the tooling in place, we could observe what was actually happening during
the mutex test hang.  The flight recorder showed context switches between the
four threads, but one thread's PID simply stopped appearing — it had been
scheduled out and never rescheduled.

### How threads sleep on a mutex

musl's `pthread_mutex_lock` eventually calls `futex(addr, FUTEX_WAIT, val)`.
The kernel's `sys_futex` creates or retrieves a `WaitQueue` for that address,
then calls `sleep_signalable_until`.  Here is the original code:

```rust
pub fn sleep_signalable_until<F, R>(&self, mut sleep_if_none: F) -> Result<R>
where F: FnMut() -> Result<Option<R>>
{
    loop {
        // ← WINDOW OPENS HERE
        current_process().set_state(ProcessState::BlockedSignalable); // (1)
        // ← LAPIC PREEMPT CAN FIRE HERE
        {
            let mut q = self.queue.lock();
            q.push_back(current_process().clone());          // (2)
            self.waiter_count.fetch_add(1, Ordering::Relaxed);
        }
        // …
        switch();
    }
}
```

### The race

On x86_64, `SpinLock::lock()` calls `cli` before spinning, disabling hardware
interrupts.  The LAPIC preemption timer fires as an interrupt.  So:

```
Thread A on CPU 1:
  set_state(BlockedSignalable)          ← removed from run queue
  [LAPIC timer IRQ fires — IF=1 here]
    → CPU 1 enters x64_handle_interrupt
    → LAPIC_PREEMPT_VECTOR handler
    → handle_ap_preempt() → switch()
    → switch() reads prev_state == BlockedSignalable
    → BlockedSignalable ≠ Runnable, so does NOT re-enqueue thread A
    → switches to thread B
  [IRQ returns — thread A is suspended mid-function]

Thread A, when eventually rescheduled to a CPU:
  push_back(current_process())          ← thread A is now in WaitQueue
```

But by the time thread A resumes and calls `push_back`, thread B may have
already released the mutex and called `wake_all()` on the WaitQueue.
`wake_all` finds an empty queue (thread A hasn't pushed yet) and returns.
Thread A then pushes itself into the WaitQueue and goes to sleep — with nobody
left to wake it.  The mutex call that would wake it has already happened.

The thread is now permanently lost: not in any scheduler queue (because
`set_state(BlockedSignalable)` removed it), not in the WaitQueue (it arrived
after `wake_all`).  Any thread waiting for it — via `pthread_join` — blocks
forever.

### The fix

Hold the WaitQueue's `SpinLock` across both `set_state` and `push_back`.
`SpinLock::lock()` calls `cli`, so the LAPIC timer cannot fire between the two
operations.  They are atomic with respect to preemption:

```rust
{
    let mut q = self.queue.lock();    // ← cli
    current_process().set_state(ProcessState::BlockedSignalable);
    q.push_back(current_process().clone());
    self.waiter_count.fetch_add(1, Ordering::Relaxed);
}   // ← sti (SpinLock Drop restores IF)
```

Now the wake-versus-sleep ordering is guaranteed: either the thread is in the
WaitQueue before `wake_all` runs (and will be woken), or `wake_all` runs first
and the thread will re-check the condition in `sleep_if_none` on the next
iteration (and return without sleeping).

A secondary fix in the early-return paths of `sleep_signalable_until`: where
the condition is already met (so we don't actually need to sleep), the original
code called `resume()` on the current process.  `resume()` sets state to
`Runnable` and then enqueues the process in the scheduler — but the process is
already running, so it ends up in the scheduler queue twice.  The fix is to
call `set_state(Runnable)` directly, which changes the state without
re-enqueueing.

### Lock ordering

The fix holds `queue.lock()` while calling `set_state`, which takes no other
locks.  `wake_all()` holds `queue.lock()` while calling `resume()`, which
acquires `SCHEDULER.lock()`.  `switch()` acquires `SCHEDULER.lock()` and does
not touch the WaitQueue.  So the ordering `queue → SCHEDULER` is consistent and
deadlock-free.

---

## Results

After the WaitQueue fix:

```
=== Kevlar M6 Threading Tests (4 vCPUs) ===

TEST_PASS thread_create_join
TEST_PASS gettid_unique
TEST_PASS getpid_same
TEST_PASS shared_memory
TEST_PASS atomic_counter
TEST_PASS mutex
TEST_PASS tls
TEST_PASS condvar
TEST_PASS signal_group
TEST_PASS tgkill
TEST_PASS mmap_shared
TEST_PASS fork_from_thread

TEST_END 12/12
```

All four safety profiles (fortress, balanced, performance, ludicrous) compile
cleanly with the flight recorder and serial dump active.

---

## What's next

With solid crash-time diagnostics and the WaitQueue race fixed, the SMP
threading substrate is stable enough to build on.  Next: TLB shootdown
infrastructure.

When one thread unmaps a page, the page-table change is immediately visible to
the kernel (via the straight-mapped physical window), but peer CPUs may have
the old translation cached in their TLBs.  Any access through a stale TLB
entry is undefined behaviour — either a silent wrong-address read or a
spurious page fault.

Phase 4 will implement the IPI-based shootdown protocol: the unmap path sends
`TLB_SHOOTDOWN_VECTOR` to all peer CPUs, each peer executes `invlpg` (or
reloads CR3 for a full flush), and the sender spin-waits until every target has
acknowledged.

| Phase | Description | Status |
|-------|-------------|--------|
| M6 Phase 1 | SMP boot (INIT-SIPI-SIPI, trampoline, MADT) | ✅ Done |
| M6 Phase 2 | Per-CPU run queues + LAPIC timer preemption | ✅ Done |
| M6 Phase 3 | clone(CLONE_VM\|CLONE_THREAD), tgid, futex wake-on-exit | ✅ Done |
| M6 Phase 3.5 | SMP debug tooling + WaitQueue race fix | ✅ Done |
| M6 Phase 4 | TLB shootdown + SMP thread safety | 🔄 Next |
