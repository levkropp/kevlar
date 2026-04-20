## Blog 193: per-thread preempt_count is a precondition for broad sti

**Date:** 2026-04-20

Continuing from [blog 192](192-allocator-irq-safety.md), which closed
the allocator-reentrancy deadlock: broad `sti` in syscall_entry still
wouldn't land.  This post documents the per-thread preempt_count
rework — a precondition that was blocking the fix, the regression it
introduced on first attempt, and how it finally went in cleanly.

## What per-CPU preempt_count couldn't do

Kevlar's `CpuLocalHead.preempt_count` lives in the CPU-local
structure pointed to by GS.  That's architecturally correct for a
per-CPU scheduler bookkeeping counter *if* you can guarantee no
thread migration ever happens with it nonzero.  Under the current
IF=0 syscall model that's true by construction — the timer can't
fire until the syscall returns, so nothing ever migrates mid-syscall.

Broad sti breaks that invariant.  Now:

- A syscall body increments `preempt_count` (e.g., via
  `SpinLock::lock_no_irq` or `SpinLock::lock_preempt`).
- A timer IRQ fires (IF=1 is allowed now).
- The ISR can't reschedule (`in_preempt()` returns true) but sets
  `need_resched`.
- The syscall's next `preempt_enable()` picks up `need_resched` and
  calls `switch()`.
- `switch()` runs `do_switch_thread`, which saves `prev`'s stack
  pointer and loads `next`'s.  But `preempt_count` stays in GS,
  pointing now at the *new* thread's view.  If `next` happens to
  have been scheduled out while holding nothing — `preempt_count`
  should be 0 — but `prev`'s artificially-elevated count remains.

The symptom: `need_resched` cleared by `switch()`, `preempt_count`
left at 1 on the CPU, so the next timer tick finds `in_preempt()==true`,
sets `need_resched`, returns without switching.  Every tick, forever.
Livelock at the timer granularity.

Linux solves this by making `preempt_count` a thread field
(`thread_info->preempt_count`) saved and restored across
`switch_to`.  Kevlar needed the same.

## First attempt — regressed XFCE

The naive patch: push `preempt_count` onto the saved stack frame in
`do_switch_thread`, pop it back on restore.  The old 8-slot frame
(rbp, rbx, r12-r15, rflags, ret_addr) grows to 9 (preempt_count
between rflags and ret_addr), and the four `push_stack` call sites
in `task.rs` (`new_kthread`, `new_user_thread`, `fork`, `new_thread`)
gain a `0` for the initial count.

Threads still passed 14/14.  XFCE started failing with `TASK CORRUPT
(PERSISTENT): pid=1 saved_rip=0x0` after nanosleep.  The
suspended-task scanner was reading rip from `rsp+56` — the old
`ret_addr` offset, now occupied by `preempt_count` which equals 0.

## The audit that made the re-attempt work

Spawned an Explore agent to catalog every reader of a suspended
task's saved stack.  Three hits:

1. `platform/x64/task.rs:86,94` — `saved_context_summary`: reads
   saved RIP from `rsp + 7*8` for the task-corruption scanner.
2. `platform/x64/task.rs:645` — guard-zone check in `switch_task`:
   reads all 8 slots into a fixed array and validates `ret_addr ==
   slots[7]`.
3. `kernel/process/process.rs:3575` —
   `scan_suspended_task_corruption`: indirects through (1).

All three had to shift from the 8-slot to the 9-slot layout.  Offsets
moved from `rsp+56` → `rsp+64` for `ret_addr`, the guard array grew
to `[u64; 9]`, the panic format string got a `preempt` field.

## Second regression, second audit

Applied — threads still passed, XFCE now hit `RIP=ffff80003e004b7c`
with an unmapped-instruction fault.  The per-thread preempt_count
change had introduced a second subtle bug I'd missed:
`userland_entry` and `forked_child_entry` both hand-rolled a
`dec gs:[GS_PREEMPT_COUNT]` near their entry points.

Under per-CPU semantics, that was correct: `switch()` called
`preempt_disable()` before `do_switch_thread`, so the new thread
inherited `preempt_count=1` on the CPU and needed the decrement.

Under per-thread semantics, `do_switch_thread` now restores the
*thread's* stored preempt_count.  For a fresh thread that value is
`0`.  Decrementing 0 as u32 produces `0xFFFFFFFF` — every
subsequent `in_preempt()` returns true forever — no thread can ever
be preempted again — livelock.

Both hand-rolled decs removed.  `do_switch_thread` is the single
point where preempt_count is stored and restored.

## The slot-order detail

After fixing the dec, the layout also needed a small tweak: pop
`preempt_count` *before* `popfq`.  If preempt is popped after,
`popfq` re-enables IF between the point where preempt_count is
stale (still reflecting prev) and the point where it's current.  A
timer IRQ fired in that window observes `in_preempt()` on prev's
basis, which may or may not match reality.

Pushing preempt_count immediately after `pushfq` (so it's popped
first on restore, before popfq runs) closes the window.

## What landed

`b38f860 preempt_count: per-thread via save/restore across do_switch_thread`

- 9-slot saved frame (preempt_count between rflags and callee-saves).
- `do_switch_thread` saves/restores preempt_count; restore happens
  before popfq.
- `userland_entry` and `forked_child_entry` no longer dec
  preempt_count manually.
- The three suspended-task-stack readers updated to the new layout.
- Initial stack layouts in `new_kthread`, `new_user_thread`, `fork`,
  `new_thread` all include `preempt_count = 0` at the right slot.

**test-threads-smp: 14/14.  test-xfce baseline (broad sti NOT applied):
3/3 runs complete, scores 4/4, 4/4, 3/4** — slightly better than the
prior 1-4/4 variance, suggesting the per-thread counter was also
quietly mis-accounting preempts in some paths that didn't exhibit
as a crash but did as scheduler weirdness.

## What's still blocking broad sti

Re-applied broad sti with the full fix stack; XFCE hit at least one
`panicked at atomic_refcell:129 already mutably borrowed` at
`switch::switch+0x10ca`.  Backtrace: sys_poll → switch().  The
"already mutably borrowed" is from
`kevlar_platform::logger::Logger::filter`, reached via the secondary
panic path — meaning *something else* panicked first, and the
secondary warn!() call re-entered the logger while a previous
format! still held the borrow.

Not landed this turn.  The remaining regression is in the switch
path under poll-wakeup racing the new preempt_count save/restore.
Filing for a future turn with fresh eyes — the foundation is solid
now (all the per-CPU-ism has been laundered out of preempt_count,
allocators are IRQ-safe, nanosleep is race-free), the remaining
issue is concentrated.

## Where the fix stands (commits, cumulative)

| | |
|---|---|
| `007ebcb` | `lock_no_irq` preempt_disable (closes STACK_REGISTRY deadlock) |
| `f84193e` | per-syscall latency histogram + `--nmi-on-stall` harness |
| `6af7fde` | `nanosleep` holds TIMERS across `set_state` |
| `4031f2c` | page_allocator + stack_cache + global heap IRQ-safe |
| `b38f860` | preempt_count per-thread |

All five landed, zero regressions on `test-threads-smp` or the
baseline `test-xfce`.  Broad `sti` itself in `syscall_entry` is the
single remaining uncommitted patch — needs one more layer of
deadlock peeled.
