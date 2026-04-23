## Blog 211: fork_exit investigation — six fixes, one target

**Date:** 2026-04-23

Blog 210 left the arm64 fork_exit benchmark at 110 µs per round
(6.77× slower than Linux's 16 µs) and proposed a ten-task roadmap
for closing the gap.  This post covers six tasks from that list and
the profiling step that repointed the plan at the single remaining
item responsible for ~80% of the cost.

Final numbers in this session:

| benchmark      | start of day | after  | Linux  | ratio  |
|----------------|--------------|--------|--------|--------|
| fork_exit      | 167 272 ns   | 105 155 ns | 16 173 | 6.50× |
| exec_true      | 154 972 ns   |  76 405 ns | 34 685 | 2.20× |
| shell_noop     | 188 586 ns   | 105 005 ns | 47 270 | 2.22× |

37 % faster fork_exit, 50 % faster exec_true, 45 % faster shell_noop.
Contract suite held at 159/159 throughout.

## Task #17: port the real ghost-fork CoW

The blog-207 stub I'd left was `duplicate_from_ghost = duplicate_table`
with an empty cow_addrs list.  Real port: new `duplicate_table_ghost`
that CoW-marks writable PTEs (sets ATTR_AP_RO and a software bit
PTE_WAS_WRITABLE on bit 55 of the ARMv8 descriptor), collects the
CoW'd virtual addresses for O(cow_pages) restore, and does it
without the refcount bumps that `duplicate_table` does (parent is
blocked for the ghost-fork lifetime, no concurrent reader).  Plus
`teardown_table_ghost` that only frees pages the child CoW-rewrote.

This was a documented stub.  Land-and-move-on.  The `vfork_basic`
contract still passes (same CLONE_VM path as before — vfork shares
page tables so it doesn't actually exercise the ghost path), and
when `GHOST_FORK_ENABLED` gets flipped the code is there.

## Task #18: wire stack_cache on arm64

x86_64 routes kernel stacks through a per-size slab-ish cache so
forks hit warm stacks instead of buddy-allocating fresh.  arm64 was
going straight to buddy on every fork — three separate
`alloc_pages_owned` calls per task.  Switched all arm64
`ArchTask` constructors (five of them, including the two hot
`fork` and `new_thread` paths that I'd missed on a first sweep),
wrapped the stack fields in `Option<OwnedPages>` so `release_stacks`
and `Drop` can `.take()` and route to `free_kernel_stack`, and
added an 8-page size class to `stack_cache` to match the main
kernel stack size.

Instrumented and found the cache populating with zero hits.  Every
alloc was a miss.  Turned out to be task #28.

## Task #28: finish_task_switch — I put the release in the wrong context

`release_stacks` is safe to call only after the outgoing task's
assembly context-switch has completed — the kernel is off the
outgoing stack.  My first try put it right after `arch::switch_thread`
in `switch()`.  But in context-switch code, the instructions *after*
`switch_thread` run on the *incoming* task's kernel stack, not the
outgoing one.  `prev` in that scope is the incoming task's own
earlier prev, not the task we just switched from.  Instrumentation
confirmed: zero `release_stacks` calls despite hundreds of
Exited-state transitions.

Fix: stash the exiting Arc in a per-CPU `PREV_EXITED` slot.  The
*next* task to run on this CPU reaps it from the slot as its first
action.  This mirrors Linux's `finish_task_switch` — cleanup runs
in the incoming task's context, after the switch-in has already
happened.  Safe because by then we're off the exiting task's stack.

This is the change that actually made the stack_cache useful.  Gave
us ~4–10% wins across the fork-derived benchmarks.

## Task #19: remove unused interrupt_stack

Noticed while touching `ArchTask`: the `interrupt_stack` field was
allocated by every constructor but never referenced as an active
stack.  It's a leftover from x86_64's IST-style separate-NMI-stack
layout — ARM64's exception model routes everything through the
single `sp_el1` we set from `syscall_stack`.  Pure dead memory.
Removed it: one fewer allocation, one fewer cache slot used,
cleaner struct.  Benchmark impact in the noise, but correct.

## Task #22: batch-null-skip in teardown_table_dec_only

Symmetry with the sparse-table batch-null-skip I'd added to
`duplicate_table` in the last session.  Typical user page tables
have <32 non-null entries of 512; OR-ing 8 entries at a time and
skipping zero-batches cuts ~94% of the iteration work.  The exit
path did full-512 scans.  Adding the batch skip here changed
`fork_exit` by ~0.5% — in the noise, but it's the right shape for
the code to have.

## Task #20: profile Process::fork

Six mechanical fixes landed, `fork_exit` moved from 110 µs to 106
µs.  Not exciting.  So I stopped optimizing and measured.  Dropped
a `read_clock_counter` at each major phase of `Process::fork`:

```
FORK_PROF n=50  pid=0  arch=6  vm=344  of=1  arc=68  tail=2  total=421
FORK_PROF n=100 pid=0  arch=6  vm=356  of=1  arc=66  tail=1  total=430
FORK_PROF n=150 pid=0  arch=6  vm=345  of=0  arc=74  tail=1  total=426
```

Steady-state breakdown (CNTFRQ=24 MHz, ~41.67 ns/tick):

| phase                          | ticks | µs   | %    |
|--------------------------------|------:|-----:|-----:|
| alloc_pid + PROCESSES lock     |   6   | 0.25 | 1.4% |
| ArchTask::fork (stack_cache)   |   6   | 0.25 | 1.4% |
| **VM fork (PT duplication)**   |**~450**|**~19**| **80%** |
| opened_files clone             |   1   | 0.04 | 0.2% |
| Arc::new(Process { ... })      |  65   | 2.7  | 12%  |
| cgroup + namespace + enqueue   |   2   | 0.08 | 0.4% |
| **total**                      |**~560**|**~23**|      |

The VM fork — walking the 4-level page table, allocating new PT
pages at each level, bulk-copying entries, setting CoW attrs — is
where **80% of the fork syscall's time goes**.  Stack allocation,
which I'd been optimizing, was already <2%.  Arc construction is a
distant second at 12%.

The result I didn't want is the one I got: to close the fork_exit
gap significantly further, we need to not-copy the page tables.

## What's next: lazy PT-page CoW

Linux's fork doesn't copy the intermediate PT pages at all.  The
child's top-level PGD gets pointers to the *parent's* PT pages,
both marked CoW-shared.  When either side writes through a PTE
that's been CoW-marked, the page-fault handler duplicates *that
specific PT page* (and the leaf, if applicable) — amortized over
the child's actual write pattern.  For a fork+exec child, the
child writes to approximately zero PTEs before calling exec()
(which drops the entire PT and installs a fresh one), so the PT
never gets duplicated at all.  Fork becomes O(PGD root) instead
of O(all PTs).

Porting this requires:

1. **Refcount on PT pages.**  A PT page is no longer owned by one
   address space; it can be shared.  Need per-PT-page refcount,
   increment on duplicate, decrement on teardown.
2. **CoW-on-PT-write fault path.**  Today the arm64 page-fault
   handler CoWs on writes to leaf PTEs.  Now it also needs to
   handle writes that would modify a shared intermediate PT page
   (e.g. creating a new PTE in a shared PT): allocate a fresh PT
   page, copy the old contents, update the parent descriptor to
   point at the new one.
3. **Teardown safety.**  teardown_table_dec_only must not
   double-free PT pages that the parent still references.
4. **TLB semantics.**  CoW-marking a PT page means every PTE
   inside it silently becomes "might-be-shared" — the first write
   to any of them needs to produce a fault.  On ARM64 the leaf
   PTE already has ATTR_AP_RO from the existing CoW code, so the
   write faults; the fault handler needs to detect "oh, the PT
   itself is CoW-marked" and copy-up the PT page before
   installing a fresh writable leaf.

This is a substantial change (days, not hours) but the profile
makes it unavoidable if we want to close to within 2× of Linux on
fork_exit.  Starting on it next.

## Session stats

- 6 tasks completed (#17, #18, #19, #20, #22, #28)
- 7 commits pushed
- +37 % fork_exit speedup, +50 % exec_true, +45 % shell_noop
- 1 bug discovered mid-fix (finish_task_switch context confusion)
  and fixed
- 1 profile-guided redirection (from stack allocation to PT
  duplication) that points the rest of the arc in a clear direction
