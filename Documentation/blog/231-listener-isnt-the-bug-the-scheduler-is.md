## Blog 231: the listener isn't the bug — the scheduler is

**Date:** 2026-04-25

Blog 230 closed `make test-i3 ARCH=arm64` at 7/7 with a workaround
(pre-warm xsetroot+xterm before i3 starts; strip i3's `exec`
autostarts so i3-startup doesn't trigger a connect-burst).  The
workaround sidesteps an "AF_UNIX listener starvation" symptom we
saw — Xorg accepts the first 2-3 connections, then never accepts
again, no matter how long we wait.

The simplest tight reproducer (`testing/contracts/sockets/listener_burst.c`:
listener + epoll(EPOLLIN) + 8 concurrent connecting children +
parent's accept loop) passes 8/8 on Kevlar arm64.  So the kernel's
listener-accept primitive is sound in isolation.  The bug only
manifests when Xorg's libev is in the loop.  That left two
hypotheses: (a) Xorg-libev specific, our kernel is fine; (b) some
indirect kernel bug only triggered by libev's syscall pattern.

This post tracks down (b) — and the root cause turns out to have
nothing to do with AF_UNIX listeners at all.  It's a **scheduler
fairness bug compounded by an arm64 AP virtual-timer death** that
the listener case just happens to expose visibly.

## Round 2: turn on the lights

Round 1 added per-listener `inode_no`, `UnixSocket::poll`,
`enqueue_connection`, and `accept` instrumentation.  Round 2
extended that with:

- **`EPOLL_TRACE_FD` cmdline** (`epoll-trace-fd=5`).  Every
  iteration of `collect_ready` or `collect_ready_inner` that
  visits the chosen fd or fd+1 logs `pid=N fd=N ev=… status=…
  ready=…`.
- **`WAITQ wake_all queue=… woken=N pids=[…]`** when the trace fd
  is set, so we see who actually got woken.
- **Per-CPU `PER_CPU_TICKS`** counter, exposed in every `TICK_HB`
  heartbeat as `per_cpu=[N0, N1, …]`.

Re-running the original failing case (i3 with autostart) with the
trace, the chronological log was:

```
AF_UNIX enqueue: listener_inode=2 bl=1 pid=8   POLL_WQ.waiters=0
AF_UNIX accept fastpath: listener_inode=2 bl_after=0
AF_UNIX enqueue: listener_inode=2 bl=1 pid=35  POLL_WQ.waiters=0
AF_UNIX accept fastpath: listener_inode=2 bl_after=0
AF_UNIX enqueue: listener_inode=2 bl=1 pid=37  POLL_WQ.waiters=0
AF_UNIX accept fastpath: listener_inode=2 bl_after=0
AF_UNIX enqueue: listener_inode=2 bl=1 pid=39  POLL_WQ.waiters=0
AF_UNIX accept fastpath: listener_inode=2 bl_after=0
AF_UNIX enqueue: listener_inode=2 bl=1 pid=41  POLL_WQ.waiters=1
AF_UNIX enqueue: listener_inode=2 bl=2 pid=42  POLL_WQ.waiters=3
AF_UNIX accept fastpath: listener_inode=2 bl_after=1
AF_UNIX accept fastpath: listener_inode=2 bl_after=0
AF_UNIX enqueue: listener_inode=2 bl=1 pid=51  POLL_WQ.waiters=3
[no accept ever fires for pid=51]
[no further pid=4 epoll iterations]
```

Important: most accepts happen normally.  The "starvation" we'd
been chasing isn't a structural listener bug; the failure is the
*last* connect (xprop, pid=51) timing out.  After pid=51 enqueues,
**Xorg (pid=4) is never scheduled again**.

## The scheduler-state snapshot

The PID 1 stall detector fired with this snapshot:

```
PID1_STALL: tick=2350 last_run=1774 gap=11520ms
PID1_STALL queues=[(3, [PId(39), PId(43), PId(7)]), (0, [])]
```

CPU 0's run queue has 3 runnable processes (pid=39 is i3, 43 is
i3bar, 7 is something else); CPU 1's queue is empty; **Xorg
(pid=4) is in neither queue** — it's `Blocked` somewhere with no
one to wake it.

A snapshot of the per-CPU tick counter at the same moment:

```
TICK_HB: cpu=0 tick=2300 ... per_cpu=[2097, 204, 0, 0, ...]
```

CPU 0 has ticked ~2100 times.  CPU 1 ticked **204 times** and then
stopped.  204 ticks at TICK_HZ=50 = ~4 seconds — almost exactly
when the connection storm hits.

Re-running the same workload twice more, sometimes CPU 0 dies and
CPU 1 keeps ticking, sometimes the reverse.  Whichever CPU happens
to be running the busy user code at the wrong moment loses its
timer.

So the real bug is **arm64 AP virtual-timer death under specific
user-code patterns**, not an epoll/listener issue.  And it's
compounded by a scheduler bug that piles wake-ups on the dying
CPU.

## Bug 1: scheduler `enqueue` doesn't load-balance

`kernel/process/scheduler.rs::enqueue`:

```rust
fn enqueue(&self, pid: PId) {
    let cpu = cpu_id() as usize % MAX_CPUS;
    self.run_queues[cpu].lock().push_back(pid);
    ...
}
```

Wakes-from-syscall always landed on the *caller's* CPU.  When
Xorg's main loop on CPU 0 woke i3, i3status, xterm, and friends
in tight succession, every wake went to CPU 0's queue.  CPU 1's
queue stayed empty.  Work-stealing only pops from the BACK of a
remote queue, so wake-ups stuck behind CPU-bound threads on CPU 0
waited a full preemption quantum (≥30 ms) before CPU 1 could
notice them.

The boosted variant `enqueue_front` already did the right thing
(picks the least-loaded CPU's queue) — but regular `enqueue`
didn't, even though the comment on `enqueue_front` warned exactly
this would happen.

Fix: copy the load-balance logic from `enqueue_front` into
`enqueue`.  One-screen change.

## Bug 2: arm64 IRQ handler EOIs the GIC AFTER the dispatch

This is the timer-death root cause.

`platform/arm64/interrupt.rs::arm64_handle_irq`, before the fix:

```rust
match irq_id as u8 {
    TIMER_IRQ => {
        timer::rearm();
        handler().handle_timer_irq();   // ← may call process::switch()
    }
    ...
}
gic::end_interrupt(irq);                 // ← AFTER dispatch
```

`handle_timer_irq` can call `process::switch()` to transfer the
CPU to a different process when a preemption tick lands.  Switch
is a one-way cooperative trip: it saves the current thread's SP
into its `ArchTask::sp` slot, loads the next thread's SP, returns
into the next thread's saved LR.  When the original thread is
later picked again, the same do_switch_thread routine restores
its SP and returns to its caller — eventually unwinding all the
way back through `handle_timer_irq` → `arm64_handle_irq` →
`gic::end_interrupt(irq)`.

That works *as long as the parked thread is eventually
rescheduled.*  If it gets killed in the meantime (`exit_group`,
fatal signal, parent SIGKILL) — and under the i3 autostart
storm there's a lot of process churn — its kernel stack is
freed and the saved IRQ frame is leaked.  `end_interrupt` is
*never called* for that one timer IRQ.  GICv2 keeps the timer
IRQ marked Active on that CPU; subsequent timer IRQs queue at
most one Pending sample and then are dropped on the floor.
**The CPU stops getting timer IRQs forever.**

That's exactly the symptom: per-CPU tick counter freezes at the
specific moment a switch happened to a soon-to-die process.
After that, the CPU is in WFI with no scheduled wakeup; the
scheduler's idle loop sees the runnable list isn't empty, but
`pick_next` has nothing in the local queue and work-stealing
needs the CPU to *get out of WFI* to even try.

Fix: EOI before the dispatch.

```rust
TIMER_IRQ => {
    timer::rearm();
    gic::end_interrupt(irq);     // ← BEFORE dispatch
    handler().handle_timer_irq();
    return;
}
```

Now even if the switch parks the IRQ frame on a doomed process,
the GIC was already cleaned up.  The timer keeps ticking on
that CPU.

This pattern — defer/early EOI before any path that may switch —
is exactly what Linux arm64 does: `irq_handler` calls
`gic_handle_irq` which immediately writes `ICC_EOIR1_EL1`
*before* invoking the per-IRQ dispatch.  Linux's implementation
encodes the same insight in the GICv3 architectural EOImode bit
(write to EOIR auto-deactivates), but the principle holds for
GICv2 too: never carry an un-EOI'd IRQ across a context switch.

## Bug 3 (proactive): no IPI infrastructure on arm64

`platform/arm64/mod.rs::broadcast_halt_ipi` was a `// TODO`
no-op.  No way to wake an idle CPU from another CPU.  When the
scheduler enqueues work to a remote CPU, the target had to wait
for its own next timer tick (~20ms) to notice the new entry.

Implemented `gic::send_reschedule_ipi(cpu)` (writes `GICD_SGIR`
with `CPUTargetList=1<<cpu`, `SGIINTID=1`), wired SGI 1 into
`arm64_handle_irq` (empty handler — the WFI exit on entry to the
IRQ vector is the wakeup itself), enabled the SGI on every CPU's
banked `ISENABLER0`, and exposed `send_reschedule_ipi` through
the platform abstraction.  `Scheduler::enqueue` and
`enqueue_front` now send a reschedule IPI to the chosen CPU when
it isn't us.

## What got better

Test results on arm64 KVM/HVF, all with `-smp 2`:

| Config | Before | After EOI fix + IPIs |
|---|---|---|
| Workaround (pre-warm xsetroot+xterm, no i3 autostart) | 7/7 | 7/7 |
| Stress (i3 autostart, original connection storm) | 4/7 | 5-6/7 |

Workaround stays robust.  Stress mode no longer suffers from the
timer death — both CPUs tick through the entire run now
(`per_cpu=[1700, 1700]` instead of `[200, 3000]`) — but the
test is still flaky 5-6/7 due to a *different* bug (xprop and
i3status occasionally don't get to where they need to be in the
test window).  That's the next round.

What's permanent and worth keeping:

- **EOI-before-dispatch** in `TIMER_IRQ` path.  Real correctness
  fix.  Linux arm64 does the same.
- **`Scheduler::enqueue` load-balances.**  Mirrors
  `enqueue_front`'s already-correct behaviour.
- **GICv2 SGI infrastructure.**  `send_reschedule_ipi`,
  `broadcast_halt_ipi`, the SGI handler, per-CPU SGI enable.
  Also unblocks future TLB-shootdown IPIs and panic-stop IPIs.
- **`idle()` defensive timer-rearm.**  Survives one specific
  class of "timer got into a bad state" failure even after the
  EOI fix, just in case.
- **`PER_CPU_TICKS` heartbeat** — diagnostic that made finding
  the EOI bug possible in the first place.
- **`EPOLL_TRACE_FD` cmdline** — opt-in trace of every event
  dispatched for a given fd.

## Closing

The "AF_UNIX listener starvation" frame from blog 230 was
misleading: every plausible thing about listeners works in
isolation.  The actual failure is a scheduler that didn't
load-balance plus an `arm64_handle_irq` that EOI'd the GIC after
the dispatch — both fixed now, both with cross-arch precedent in
how Linux does it.

The deeper lesson: when a kernel symptom presents in subsystem A
(epoll/listener) but the actual cause lives in subsystem B (IRQ
handling), the only way through is to make subsystem B *visible*.
This round shipped four pieces of permanent diagnostic
infrastructure (`PER_CPU_TICKS`, `EPOLL_TRACE_FD`, the `WAITQ
wake_all` PID log, and proper `socket:[N]` / `pipe:[N]` /
`anon_inode:[N]` in `/proc/<pid>/fd`).  All four together pointed
straight at the late-EOI bug in under an hour, after spending
days chasing the wrong subsystem with neither.

This kind of misdirection — symptom in subsystem A, root cause
in subsystem B — is why the kernel-side `strace-pid=N` and the
new `epoll-trace-fd=N` cmdline trace toggles matter.  Both
specifically exist to disprove the *first* hypothesis fast and
let you find the second.
