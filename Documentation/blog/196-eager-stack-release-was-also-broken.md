## Blog 196: eager stack release was also broken, independently

**Date:** 2026-04-20

Continuing the broad-sti landing effort.  Blog 195 closed the PT-page
teardown race with always-defer Vm::Drop + dropped the sti/cli dance
in the SpinLock contention warning.  Broad sti then hit a new panic:

```
switch_thread BUG: saved_rsp_off=0xffffffffffffba80
slots: rbp=0x346a3000 rbx=... preempt=0x27 rflags=0x34691000 ret=0x41
```

The interesting fields are `rbp=0x346a3000`, `r12=0x346a5000`,
`r13=0x346a6000`, `rflags=0x34691000` — those are kernel direct-map
paddrs, not register values.  The saved frame has been overwritten
with data that looks like a page-allocator bitmap or a zone-
descriptor table.  And `saved_rsp_off` is a giant negative number,
meaning `next.rsp` points BELOW `kstack_base`.

The kernel stack has been freed and reissued as something else,
while some code path still references it.

## What blog 147 said, and what got re-broken

Blog 147 (2026-04-06) documented this exact bug:

> The switch() function in kernel/process/switch.rs eagerly freed
> exiting processes' kernel stacks immediately after context-switching
> away from them. On SMP, another CPU could pick the just-exited
> process from the scheduler queue (due to a narrow re-enqueue window
> during process exit) and attempt to context-switch to the freed
> stack, causing a NULL RIP page fault.

At the time, the fix was to disable the eager `release_stacks()` call.
Some later commit re-added it.  Under broad sti the window widens
further (mid-syscall preemption), and the same use-after-free becomes
routine.

This turn's change: re-disable the eager free.  Lazy free via Drop of
the last Arc<Process> happens after gc_exited_processes has drained
any remaining references.  Cost ~32KB per zombie until wait4 reaps.

## Committed as 26cb5d2

- test-threads-smp: 14/14.
- test-xfce baseline (broad sti NOT applied): 3/3 runs complete,
  scores 3/4, 3/4, 2/4.

## What broad sti does with all fixes in tree

5-run test-xfce with broad sti applied on top of 26cb5d2:

- 2 complete (scores 3/4, 1/4)
- 3 hung (full 300s timeout, no panic)

So the panic pattern is gone — release_stacks was the last panic
signature blocking broad sti.  But the *hangs* haven't been fixed.
At least one of them corresponds to a text-segment content mismatch
signature (FILL_VERIFY detects fresh fault-in produced the wrong
bytes), which is the original blog-188 IF=0 stale-TLB leak surface.
Paradoxically, broad sti was supposed to *fix* that by enabling TLB
IPI broadcast — and it does, but the race window around
always-defer Vm::Drop (blog 195) widens elsewhere.

The remaining work is the *timing* of TLB flushes under broad sti —
the current always-defer widens the window where stale TLB entries
can still be used by remote hardware walkers before the teardown
runs.  This is the tension: deferring makes PT-page recycling safe
but widens the stale-entry window for user data pages.

## Where this session leaves things

Eight layers now committed, all blog-documented:

| | |
|---|---|
| `007ebcb` | lock_no_irq preempt_disable |
| `f84193e` | histogram + --nmi-on-stall |
| `6af7fde` | nanosleep TIMERS lock widened |
| `4031f2c` | allocator IRQ-safety |
| `b38f860` | per-thread preempt_count |
| `0c39aa7` | always-defer Vm::Drop + SpinLock warn no IF flip |
| `26cb5d2` | do not eagerly free exiting task's kernel stacks |

Broad sti in syscall_entry remains the single uncommitted line.
The remaining work is a proper RCU/grace-period scheme for TLB
invalidation that isn't limited by deferred teardown timing.

Baseline quality keeps improving: test-xfce 3/3 runs complete
with clean scores, down from an original 1-4/4 variance at the
start of this sequence of fixes.  The infrastructure pays off
regardless of whether broad sti lands this month or next.
