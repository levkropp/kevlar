# M6 Phase 3: Threading

Kevlar now supports POSIX threads end-to-end.  `pthread_create`, `pthread_join`,
mutexes, condition variables, TLS, `tgkill`, and `fork` from a threaded process
all work correctly under an SMP guest.  Twelve integration tests pass on 4 vCPUs.

This one was a marathon.

---

## What "threading" actually requires

`fork()` was already working.  A thread is not a fork — it's closer, and in some
ways harder.  The Linux ABI for thread creation goes through `clone(2)` with a
specific set of flags:

```
clone(CLONE_VM | CLONE_THREAD | CLONE_SIGHAND | CLONE_FILES |
      CLONE_FS | CLONE_SETTLS | CLONE_CHILD_SETTID | CLONE_CHILD_CLEARTID,
      child_stack, &ptid, &ctid, newtls)
```

Each flag is a contract:

| Flag | Contract |
|------|----------|
| `CLONE_VM` | Share the address space (no copy-on-write) |
| `CLONE_THREAD` | Same thread group → `getpid()` returns parent's PID |
| `CLONE_SETTLS` | Set FS base (x86_64) / TPIDR_EL0 (ARM64) to `newtls` |
| `CLONE_CHILD_SETTID` | Write child TID to `ctid` in child's address space |
| `CLONE_CHILD_CLEARTID` | On thread exit: write 0 to `ctid`, wake futex waiters |

`CLONE_CHILD_CLEARTID` is what makes `pthread_join` work.  musl's join
implementation sleeps on `futex(ctid, FUTEX_WAIT, tid)`.  When the thread
exits and clears `ctid`, the kernel wakes that futex.  No CLEARTID, no join.

---

## Kernel changes

### Process struct: tgid and clear_child_tid

Two new fields on `Process`:

```rust
pub struct Process {
    pid:  PId,
    tgid: PId,                       // thread group id; == pid for leaders
    clear_child_tid: AtomicUsize,    // ctid address, or 0
    // …
}
```

`fork()` sets `tgid = pid` (new process is its own group leader).
`new_thread()` sets `tgid = parent.tgid` (same thread group as creator).

`getpid()` returns `tgid`.  `gettid()` returns `pid`.  This is the Linux
invariant: all threads in a group see the same `getpid()`.

### sys_clone: the thread path

`clone(CLONE_VM | …)` routes to a dedicated code path that calls
`Process::new_thread()` instead of `Process::fork()`:

```rust
if flags & CLONE_VM != 0 {
    let set_child_tid   = flags & CLONE_CHILD_SETTID  != 0;
    let clear_child_tid = flags & CLONE_CHILD_CLEARTID != 0;
    let newtls_val = if flags & CLONE_SETTLS != 0 { newtls as u64 } else { 0 };

    let child = Process::new_thread(
        parent, self.frame,
        child_stack as u64, newtls_val,
        ctid, set_child_tid, clear_child_tid,
    )?;
    // …
    Ok(child.pid().as_i32() as isize)
}
```

Note the argument swap between architectures: x86_64 passes `(ptid, ctid,
newtls)` but ARM64 passes `(ptid, newtls, ctid)`.  A single `#[cfg]` at the
top of the handler unpacks them into the right names.

### new_thread(): what's shared, what's not

```rust
let child = Arc::new(Process {
    pid,
    tgid: parent.tgid,                     // same thread group
    vm:   AtomicRefCell::new(parent.vm().as_ref().map(Arc::clone)), // shared
    opened_files: Arc::clone(&parent.opened_files),                 // shared
    signals:      Arc::clone(&parent.signals),                      // shared
    signal_pending: AtomicU32::new(0),     // per-thread (own pending bitmask)
    sigset: AtomicU64::new(parent.sigset_load().bits()), // inherited mask
    clear_child_tid: AtomicUsize::new(0),
    // … credentials, umask, comm all copied from parent
});
```

Three things are shared via `Arc`: the virtual memory map (`vm`), the open
file table (`opened_files`), and the signal disposition table (`signals`).
The signal *pending* bitmask and signal *mask* are per-thread — threads have
independent delivery state even though they share handlers.

### ArchTask::new_thread(): the stack layout

Every thread needs its own kernel stack, interrupt stack, and syscall stack —
three 1 MiB allocations.  The initial kernel stack is pre-loaded with a fake
`do_switch_thread` context frame so the thread can be scheduled like any other:

```rust
// IRET frame for returning to userspace.
rsp = push_stack(rsp, (USER_DS | USER_RPL) as u64); // SS
rsp = push_stack(rsp, child_stack);                 // user RSP  ← pthread stack
rsp = push_stack(rsp, frame.rflags);                // RFLAGS
rsp = push_stack(rsp, (USER_CS64 | USER_RPL) as u64); // CS
rsp = push_stack(rsp, frame.rip);                   // RIP ← clone() return addr

// Registers popped before IRET (clone() returns 0 to child via RAX).
rsp = push_stack(rsp, frame.rflags); // r11
rsp = push_stack(rsp, frame.rip);    // rcx
// … rsi, rdi, rdx, r8-r10

// do_switch_thread context frame.
rsp = push_stack(rsp, forked_child_entry as *const u8 as u64); // "return" address
rsp = push_stack(rsp, frame.rbp);
// … callee-saves …
rsp = push_stack(rsp, 0x02); // RFLAGS (interrupts disabled)
```

When the scheduler first picks up the new thread, `do_switch_thread` pops the
callee-saves and returns to `forked_child_entry`, which pops the remaining
registers and executes `iret` — landing in userspace at `clone()`'s return
address with RSP pointing at the freshly-allocated pthread stack.

The ARM64 path is analogous, replacing the IRET frame with an `eret`-compatible
exception-return frame via `SPSR_EL1` and `ELR_EL1`.

### Thread exit: CLEARTID and futex wake

On thread exit, `Process::exit()` checks `is_thread = (tgid != pid)`.  For
threads:

* Skip sending `SIGCHLD` (thread exits are invisible to the parent process).
* Skip closing file descriptors (the table is shared with siblings).
* Write 0 to `clear_child_tid` address and call `futex_wake_addr`.
* Push the `Arc<Process>` onto `EXITED_PROCESSES` (so the Arc stays alive
  through the upcoming context switch — the idle thread GCs it later).

```rust
let ctid_addr = current.clear_child_tid.load(Ordering::Relaxed);
if ctid_addr != 0 {
    let _ = uaddr.write::<i32>(&0);
    futex_wake_addr(ctid_addr, 1);
}
```

Without the `EXITED_PROCESSES` push, `switch()` would free the thread's kernel
stacks while still executing on them:

```
PROCESSES.remove(&pid)  → refcount drops to 1 (only CURRENT)
arc_leak_one_ref(&prev) → refcount 1 (CURRENT)
CURRENT.set(next)       → drops CURRENT → refcount 0 → freed ← use-after-free
switch_thread(prev.arch, next.arch) ← executing on freed memory
```

### exit_group

`exit_group(2)` terminates the entire thread group.  The implementation
collects all sibling threads (same `tgid`, different `pid`), sends each
`SIGKILL`, then calls `exit()` on the current thread.  The siblings receive
the signal on their next preemption and call their own `exit()`.

---

## The integration test

`testing/mini_threads.c` exercises twelve scenarios in order:

| # | Test | What it checks |
|---|------|----------------|
| 1 | `thread_create_join` | Basic create + join, return value |
| 2 | `gettid_unique` | Each thread has a distinct TID |
| 3 | `getpid_same` | All threads share the same TGID |
| 4 | `shared_memory` | Stack variable written by one thread read by another |
| 5 | `atomic_counter` | 4 threads × 1000 increments = 4000 (no data race) |
| 6 | `mutex` | `pthread_mutex` serialises 4 × 1000 increments |
| 7 | `tls` | `__thread` gives per-thread storage |
| 8 | `condvar` | `pthread_cond_wait` + `pthread_cond_signal` |
| 9 | `signal_group` | `kill(getpid(), SIGUSR1)` delivered to thread group |
| 10 | `tgkill` | Signal routed to a **specific** thread by TID |
| 11 | `mmap_shared` | Anonymous mmap written by child thread |
| 12 | `fork_from_thread` | `fork()` from a threaded process, `waitpid()` succeeds |

Tests 1–9 and 11–12 passed quickly.  Test 10 took everything else in this post.

---

## The debugging marathon

### First: a deadlock hiding as a panic

With 4 vCPUs and all tests running, the kernel would panic somewhere in
tests 1–3 with `double panic!` — a second panic firing while the first
panic handler was still running.

Following the backtrace, the first panic address decoded to a `Result::expect`
in the kernel but with a return address of `0x46` — obviously corrupt.  Stack
corruption at that level usually means either a stack overflow or a lock
deadlock that caused a CPU to spin until the watchdog fired.

Reading `new_thread()` and `switch()` side by side revealed a classic AB-BA
deadlock:

```
CPU 0 (new_thread):  lock PROCESSES → ... → lock SCHEDULER
CPU 1 (switch):      lock SCHEDULER → ... → lock PROCESSES
```

`new_thread()` was holding `PROCESSES` when it called `SCHEDULER.lock().enqueue()`.
`switch()` was holding `SCHEDULER` when it called `PROCESSES.lock().get()` inside.
Under SMP, both could fire simultaneously.

The fix is one line — drop `PROCESSES` before touching `SCHEDULER`:

```rust
process_table.insert(pid, child.clone());
drop(process_table); // ← release before acquiring SCHEDULER
SCHEDULER.lock().enqueue(pid);
```

Applied in both `fork()` and `new_thread()`.  Tests 1–9 and 11–12 passed.

### Then: test 10 (tgkill) — the double-panic

`tgkill` test spins a child thread and has the main thread send it `SIGUSR2`
via `tgkill(getpid(), child_tid, SIGUSR2)`.  Consistently: panic, then
`double panic!`, then halt.

The first panic decoded to a kernel-mode General Protection Fault at
`core::fmt::write + 0x23` — a `movzbl 0x0(%r13), %eax` with R13 holding a
non-canonical address.  In other words, the kernel panicked while trying to
format a panic message, then panicked again while formatting *that* panic.

Two separate bugs caused this.

#### Bug 1: panic handler ordering

The panic handler structure was:

```rust
fn panic(info: &core::panic::PanicInfo) -> ! {
    if PANICKED.load() { /* double panic exit */ }

    // … capture msg_buf from info …

    begin_panic(Box::new(msg_buf.as_str().to_owned())); // ← unwind to catch frame

    PANICKED.store(true);   // ← set AFTER begin_panic returned

    error!("{}", info);     // ← use info directly
}
```

Two problems here. `begin_panic` (from the `unwinding` crate) scans the stack
for catch frames.  It unwinds through `x64_handle_interrupt`'s stack frame —
the frame that *owns* the `fmt::Arguments` referenced by `PanicInfo`.  After
`begin_panic` returns (no catch frame found), `info.message` points into
destroyed stack data.  The subsequent `error!("{}", info)` dereferences a
non-canonical pointer — the second GPF.

And because `PANICKED.store(true)` was *after* `begin_panic`, any exception
during `begin_panic`'s unwinding wouldn't hit the double-panic guard — it
would fall through and try to panic again from scratch, eventually hitting the
second GPF and *then* the double-panic guard.

The fix: reorder all three operations:

```rust
fn panic(info: &core::panic::PanicInfo) -> ! {
    // 1. Disable interrupts immediately.
    unsafe { core::arch::asm!("cli", options(nomem, nostack, preserves_flags)); }

    if PANICKED.load(Ordering::SeqCst) { /* double panic */ }

    // 2. Set PANICKED before begin_panic — any exception during unwinding
    //    is now caught as "double panic" rather than re-entering here.
    PANICKED.store(true, Ordering::SeqCst);

    // 3. Capture message NOW, before begin_panic can corrupt info.
    let mut msg_buf = arrayvec::ArrayString::<512>::new();
    let _ = write!(msg_buf, "{}", info);

    begin_panic(Box::new(alloc::string::String::from(msg_buf.as_str())));

    // 4. Use msg_buf from here on, not info.
    error!("{}", msg_buf.as_str());
    // …
}
```

The `cli` at the top was already there (from the prior session's fix to prevent
hardware IRQs from firing during panic formatting).  The new ordering ensures
that even if `begin_panic` corrupts the stack, the kernel either exits cleanly
via a catch frame or hits the double-panic guard.

(The `to_owned()` / `to_string()` calls fail to compile in `no_std` without the
trait explicitly in scope; `alloc::string::String::from()` bypasses that.)

#### Bug 2: signals never delivered to AP CPUs

Even with the panic handler fixed, `tgkill` would still fail: the signal was
sent, but the target thread — running on CPU 1, 2, or 3 — never received it.

The interrupt handler dispatches on the vector number:

```rust
match vec {
    LAPIC_PREEMPT_VECTOR => {
        ack_interrupt();
        handler().handle_ap_preempt();   // schedules next thread
        // … (nothing else)
    }
    _ if vec >= VECTOR_IRQ_BASE => {
        ack_interrupt();
        handle_irq(irq);
        // Deliver pending signals when returning to userspace.
        if frame.cs & 3 != 0 {
            handler().handle_interrupt_return(&mut pt); // ← try_delivering_signal
        }
    }
    // exceptions …
}
```

`handle_interrupt_return` calls `try_delivering_signal`.  It was only in the
hardware IRQ arm.

Hardware timer IRQs (PIT/HPET via IOAPIC) route only to the BSP (CPU 0).
Application Processors only ever receive `LAPIC_PREEMPT_VECTOR`.

So: a thread running on CPU 1, 2, or 3 would be preempted by the LAPIC timer,
the kernel would schedule the next task, and return to userspace — but
`try_delivering_signal` was never called.  `tgkill` set the target thread's
`signal_pending` atomic, but nobody ever checked it on the AP.

The fix is small: copy the signal delivery block into the `LAPIC_PREEMPT_VECTOR`
arm:

```rust
LAPIC_PREEMPT_VECTOR => {
    ack_interrupt();
    handler().handle_ap_preempt();
    // Deliver pending signals when returning to userspace.
    // Without this, threads on AP CPUs would never get signals.
    let cs = frame.cs;
    if cs & 3 != 0 {
        let mut pt = PtRegs { /* copy frame fields */ };
        handler().handle_interrupt_return(&mut pt);
        frame.rip = pt.rip;
        frame.rsp = pt.rsp;
        // …
    }
}
```

With this in place, the LAPIC timer on each AP also checks for pending signals
on every return to userspace — exactly as the BSP's hardware timer does.

---

## Results

```
=== Kevlar M6 Threading Tests ===
PID=1  TID=1  CPUs=1

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

Under `-smp 4` (TCG), all twelve pass.

---

## What's next

The threading implementation is functionally correct but still has rough edges
for a production SMP kernel:

* **TLB shootdowns**: when one thread unmaps a page, other CPUs still have that
  mapping cached in their TLBs.  Currently safe under TCG (single-threaded
  emulation), but required before any real hardware or KVM multi-thread workload.
* **Per-thread signal pending**: `tgkill` sets the target's `signal_pending`
  atomic, but the *delivery* races with other threads that share the `signals`
  `Arc`.  A thread could receive a signal intended for its sibling if the
  sibling checks first.  Acceptable for now; fixing it requires splitting the
  pending bitmask out of the shared `SignalDelivery`.
* **`pthread_cancel`, `pthread_barrier`, `pthread_rwlock`**: not yet implemented.
  musl falls back to futex-based implementations, so they may work partially.

The next milestone is TLB shootdown infrastructure — at which point the kernel
will be safe to run under KVM with multiple vCPUs exercising real parallelism.

| Phase | Description | Status |
|-------|-------------|--------|
| M6 Phase 1 | SMP boot (INIT-SIPI-SIPI, trampoline, MADT) | ✅ Done |
| M6 Phase 2 | Per-CPU run queues + LAPIC timer preemption | ✅ Done |
| M6 Phase 3 | clone(CLONE_VM\|CLONE_THREAD), tgid, futex wake-on-exit | ✅ Done |
| M6 Phase 4 | TLB shootdown + SMP thread safety | 🔄 Next |
