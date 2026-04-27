# 242 — The loop is in musl memcpy (or is it?)

Phase 19 (blog 241) reproduces the hang from a 3-request kbox.
This session pushed kernel-side instrumentation to identify what
Xorg is doing during the hang.  Result: the loop *appears* to be
inside musl libc's `memcpy`, at offset `0x184b8` (= `memcpy + 0xa8`),
on instruction `stp x6, x7, [x0]`.

## The instrumentation

`platform/arm64/interrupt.rs` now stashes the most recent IRQ
frame pointer per-CPU in `LAST_IRQ_FRAME[]`, exposing user PC, SP,
and PSTATE via `last_user_state(cpu) -> Option<(pc, sp, pstate)>`.

`kernel/timer.rs` polls that snapshot twice per second once
`pid1_gap > 100` ticks (1s) and prints:

```
PID1_STALL: tick=4425 gap=4960ms cpu=0 EL0 user_pc=0xa0024b4b8 user_sp=0x9ffffeaa0 fp_traps=693 pf=0
```

`fp_traps` counts EC=0x07 FP-trap entries; `pf=` is
`PAGE_FAULT_COUNT`.

Cross-referencing the user PC against `/proc/4/maps` (also added
to the test's failure path) places `0xa0024b4b8` inside the
`a00233000-a002d5000 r-xp` mapping = `ld-musl-aarch64.so.1`.
`nm` confirms offset `0x184b8` is `memcpy + 0xa8`.

## What the data says

| Probe | Reading |
|---|---|
| user_pc (cpu=0) | `0xa0024b4b8` (memcpy+0xa8), constant for 5+ seconds |
| user_sp (cpu=0) | `0x9ffffeaa0`, constant for 5+ seconds |
| `fp_traps` | freezes at 693 — Xorg owns FP, no other task takes ownership |
| `pf` | 0 throughout the hang — no page faults |
| TICK_HB | both CPUs continue to tick; pid1_gap grows monotonically |
| SMP=1 | hangs identically — not migration |

Three facts to reconcile:

1. `memcpy + 0xa8` = `stp x6, x7, [x0]`.  A single store. Cannot
   loop on its own.
2. The next byte `184bc` is also a store, then `184c0`, …
   eventually a `ret`.  No backward branch.
3. PC + SP unchanged across many seconds.

The honest read: the IRQ-frame snapshot is *stale*.  The frame
pointer is set on every IRQ entry but never invalidated when the
CPU goes idle or switches to a different task at EL1.  When cpu=0
is idle and only IRQs entering at EL1 fire on it, the EL0 frame
captured during Xorg's last preemption sits in
`LAST_IRQ_FRAME[0]` indefinitely and we keep printing it.

## What this is, and isn't

It IS evidence that Xorg, *at some point*, stopped at memcpy+0xa8.
That's a real signal — Xorg's last-known userspace position when
we lost the trail.

It is NOT proof Xorg is currently looping there.  We have no
direct evidence of CPU burn — TICK_HB shows CPUs ticking, but a
ticking CPU may be idle.  The observed `pid1_gap` could just be
init's `waitpid` blocking; `init` doesn't burn CPU during the
xprop probe.

## What we'd need next

To finish locating the loop:

1. **Distinguish "user PC at last EL0→EL1 entry" from "task is
   actively running"**.  Track in the kernel which task is *on*
   each CPU right now (vs. the last user task to be interrupted),
   and only sample its PC when it's the running task.  Or better,
   thread the IRQ frame into the timer handler and capture it only
   when `frame.pstate.M[3:0] == 0` (came from EL0) AND the CPU is
   running a userspace task.

2. **Per-task FpState invariant check**.  If FP state corruption is
   ruled out (which `fp_traps` freezing during the hang weakly
   suggests), focus elsewhere.

3. **/proc/N/syscall**.  Add a procfs entry that reports the
   currently-blocked syscall (or "running"); poll it from the
   test to see whether Xorg is stuck in a syscall (and which one)
   or genuinely user-bound.

4. **Smaller C reproducer**.  If a 50-line C program issuing the
   3 X11 requests hangs identically, the trigger is fully
   instrumentable from the test side.

## Status

- Phase 19 reproducer: ✅ in-tree, deterministic, 30 LoC of Rust
- xprop hang signature on phase 19: ✅ matches real openbox
- Kernel-side memcpy theory: ⚠️ unconfirmed — IRQ frame is stale

The minimal repro stands.  The kernel-bug location does not.
