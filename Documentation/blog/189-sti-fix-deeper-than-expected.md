## Blog 189: enabling IF in syscalls — when the one-line fix isn't

**Date:** 2026-04-19

[Blog 188](188-syscall-if-zero.md) identified the root cause of task
#25's stale-TLB / kernel-pointer-leak corruption: every syscall in
Kevlar runs with IF=0, so every `flush_tlb_remote` from inside a
syscall silently degrades to "bump PCID generation, skip the IPI."
Stale TLB entries on remote CPUs survive long enough for freed paddrs
to be recycled — kernel data leaks into user pages.

This post documents the next turn of investigation: trying to land the
fix and discovering it's deeper than a single `sti`.

## What seemed straightforward

The minimum viable fix is one assembly instruction in
`platform/x64/usermode.S`, between the SWAPGS+stack-switch+pt_regs
build and the `call x64_handle_syscall`:

```asm
push rax     // syscall number
sti          // re-enable IF for the syscall body
call x64_handle_syscall
```

SWAPGS is done, the kernel stack is switched, the pt_regs frame is
built — interrupts can safely fire from this point onward.  Linux does
the equivalent.

## What actually happened

`make test-threads-smp` with this change: **14/14 tests pass.**  The
kernel is stable.  Threading, fork-from-thread, mmap_shared, signals,
pipes, futexes, atomic counters — all work with sti.

`make test-xfce` with the same change: **all 10 runs hang at the
start of Phase 5.**  The log gets to:

```
=== Phase 5: XFCE Session ===
FILL_TRACE: pid=4 vaddr=0xa0001d000 ... paddr=0x3e07b000
FILL_VERIFY: pid=4 vaddr=0xa0001d000 ... paddr=0x3e07b000
FILL_TRACE: pid=4 vaddr=0xa100f1000 ... paddr=0x3e091000
FILL_VERIFY: pid=4 vaddr=0xa100f1000 ... paddr=0x3e091000
qemu-system-x86_64: terminating on signal 15 from pid 476281 ...
```

PID 4 is the first child Phase 5 spawns (`dbus-uuidgen >
/etc/machine-id`).  It demand-faults two text pages in, then nothing.
The qemu process gets SIGTERM at the 300-second test timeout
boundary.  No NMI watchdog fires (suggesting timer interrupts have
stopped on both CPUs).  No SIGSEGV.  No panic.  Just silence.

A quick test with a syscall counter added to the timer heartbeat
showed: timer fires 200 ticks (2 seconds) after boot, then stops.
Syscalls climbed to 28 during boot, then no further syscalls
recorded.  Both timer and syscalls suspended.

## A second attempt: localized sti at the IPI

Maybe instead of changing the global syscall IF state, just enable IF
inside `tlb_remote_flush_all_pcids` for the IPI broadcast itself,
then restore.  The lock guards above are `lock_no_irq` (don't disable
IF themselves), so re-enabling locally is structurally OK.

```rust
let if_was_off = !interrupts_enabled();
if if_was_off { sti }
// ... acquire TLB_SHOOTDOWN_LOCK, broadcast IPI, spin-wait for ACK ...
if if_was_off { cli }
```

3-run xfce sample: **also hangs.**  But this time the NMI watchdog
fires:

```
NMI WATCHDOG: CPU 0 STUCK
  RIP=0xffff80000018fd63  RSP=0xffff800030edee88
  RFLAGS=0x97  (IF=0, ring 0)
  preempt_count=1  need_resched=0
  Backtrace:
    #0: 0xffff80000012ce65
    #1: 0xffff8000001553d6
    #2: 0xffff80000019d3db   handle_syscall
    ...
  lockdep: no locks held
```

`addr2line` resolves the stuck RIP:

```
<kevlar_platform::spinlock::SpinLock<kevlar_kernel::mm::vm::Vm>>::lock_no_irq
/home/fw/kevlar/platform/spinlock.rs:130
<kevlar_kernel::syscalls::SyscallHandler>::do_dispatch
/home/fw/kevlar/kernel/syscalls/mod.rs:1817
```

CPU 0 is in `lock_no_irq` waiting for the Vm lock, with IF=0 (because
the syscall body still runs with IF=0 — the localized fix only flips
IF inside the IPI helper).  Some other CPU holds the Vm lock and is
inside `flush_tlb_remote` with my localized sti=1, broadcasting an
IPI.  CPU 0 can't ACK because its IF=0.  Sender spins.  CPU 0 spins.
**Deadlock.**

So the localized fix introduces its own deadlock — broadcasting IPIs
to CPUs that can't receive them.  Without making *all* CPUs willing
to receive IPIs (i.e., the broad sti), the localized approach can't
work.

## Why the broad sti hangs XFCE but not threads

That's the open question.  The threads test exercises:

- `clone(CLONE_VM | CLONE_THREAD | ...)`
- pthread mutex / condvar (futex)
- `mmap(MAP_SHARED)`
- pipes
- signals (SIGUSR1, SIGCHLD)
- `fork()` from threads

All work with sti enabled.

XFCE adds:

- D-Bus IPC (Unix sockets, a lot of them)
- Many fork+exec pairs
- Heavier shared-library mmap pressure
- X11 (sockets, shared memory)

The hang happens at *the very first* child process — `dbus-uuidgen`,
which reads `/dev/urandom` and writes `/etc/machine-id`.  After two
demand faults for its text segment, the system stops producing log
output.  Whether it's a printk-path lock issue under contention, a
sleep/wakeup ordering issue exposed by syscall preemption, or
something else, the diagnosis needs an interactive debugger session.

## What landed this turn

Nothing in `platform/` or `kernel/`.  The broad sti is reverted.
The localized sti is reverted.  The diagnostic primitives
(KERNEL_PTR_LEAK + zero-fill verifier) remain in tree from previous
turns — they're how we'd measure success when the fix eventually
lands.

The page_allocator zero-fill verifier did get a small enhancement:
on hit, it now dumps the window of qwords around the first non-zero
offset and counts kernel-direct-map-shaped values across the page.
That improvement stays.

## What we now know

| question | answer |
|---|---|
| Is the IF=0 syscall the source of stale-TLB leaks? | Yes (blog 188 confirmed). |
| Will adding `sti` to syscall_entry alone fix it? | No.  Threads OK; XFCE hangs. |
| Will localized `sti` around the IPI fix it? | No.  Deadlocks with IF=0 receiver. |
| Is there workload-specific code that depends on IF=0? | Yes, somewhere in the path between `dbus-uuidgen` start and its first syscalls beyond demand fault. |

## Next steps

The fix needs an interactive debug session: load the kernel under a
debugger, set a breakpoint on the timer-stop point, and inspect what
each CPU is doing at the moment everything goes quiet.  Specifically:

1. Confirm whether the timer ISR has actually stopped firing or
   whether log output is the bottleneck.
2. If the ISR has stopped, find what disabled the LAPIC timer or what
   prevents the IRQ from being delivered.
3. If output is the bottleneck, identify the contended lock and
   either change its policy or batch the prints.

Either way, the work is bigger than a turn.  Filing for the next
investigation.
