## Blog 213: fork_exit investigation continued — where iteration 2 dead-ends

**Date:** 2026-04-23

Blog 212 shipped iteration 1 (leaf PT sharing) and said iteration 2
(intermediate PT sharing) was next.  This post is what I found when I
sat down to design iteration 2.  It doesn't ship code; it's a report
on why the lazy-intermediate-PT-CoW design I sketched in blog 211's
"what's next" section doesn't work the way I thought, plus a
profile-guided look at where the remaining ~84 µs of fork_exit
actually lives.

Short version: iteration 2 needs something I don't have (ASID-tagged
TLBs or a kernel rebuild without FP/NEON), or else fundamental
redesign of the invariant.  Iter-1 stays shipped.  The rest of the
fork_exit gap isn't in PT duplication at all.

## Where I thought iteration 2 was going

Leaf PT sharing (iter-1) works because the hardware gives us a gate:
every writable user PTE inside a shared PT gets `ATTR_AP_RO` stamped
at fork, and the first write from either owner faults into
`handle_page_fault`, which then calls `traverse_mut` on that Vm's
PGD.  `traverse_mut` sees `PTE_SHARED_PT` on the PMD descriptor,
allocates a fresh leaf PT, copies, updates that Vm's PMD, and
returns a pointer into the fresh PT.  The other Vm keeps its old
view untouched.  Counters stay balanced because the `ATTR_AP_RO`
flag is what forced the fault.

The obvious extension is to share PMD tables (or PUD tables) the
same way: set `PTE_SHARED_PT` on the PUD descriptor at fork,
`pt_ref_inc` the PMD, child's PGD gets a bulk-copied entry with
SHARED set.  First write through a shared PMD triggers traverse_mut
at level 2, which unshares the PMD.

The problem is the *invariant step*.  When we unshare a PMD:

```
pt_refcount(old_PMD) was 2 (shared with another Vm).
Alloc fresh PMD F.  Copy entries from old_PMD to F.
Our PUD entry → F, clear SHARED, pt_ref_dec(old_PMD)  →  1.
```

Every non-null entry in F now points at a leaf PT that the *other
Vm's* old_PMD also points at.  Each of those leaf PTs used to be
referenced by exactly one PMD entry; now it's referenced by two
(F's and old_PMD's).  We have to reflect this somehow, or
subsequent teardowns will double-free or overwrite shared leaves.

Two options, and neither is clean:

**Option A: bump leaf pt_refcounts at unshare time.**  The unshare
routine walks F, `pt_ref_inc` for each non-null entry, and *also*
walks old_PMD and marks each entry with `PTE_SHARED_PT`.  The
second walk races with the other Vm: if they're in `traverse_mut`
on old_PMD at the same instant, they might read an entry as
non-SHARED before we stamp it, treat it as sole-owner, and mutate
the leaf PT in place — while the leaf is now actually shared via F.

**Option B: traverse_mut always checks pt_refcount at each level,
ignoring PTE_SHARED_PT.**  Works, but the baseline for intermediate
levels becomes "refcount > 1 means shared" — which means the
*other Vm's* view (which didn't participate in the unshare) still
sees non-SHARED bits on its PMD entries even though those entries
now point at shared leaves.  The SHARED bit becomes a useless
hint; everything comes down to the atomic refcount read, which is
cheap but loses the fast path that was the whole point of having
the bit.

Neither option has the clean "SHARED bit ⇒ refcount > 1" invariant
that iteration 1 relies on.  The root cause is that at intermediate
levels there's no hardware fault gate: the MMU page-table walker
silently reads the PMD descriptor and descends.  We can't force a
race-free transition.

Linux's answer is ASID-tagged TLBs plus doing PT duplication
eagerly but cheaply.  Linux doesn't do lazy intermediate PT
sharing on regular fork at all — it only does CoW at the data
level.  The fast-fork path in Linux is `vfork`/`clone(CLONE_VM)`,
where child shares parent's address space outright and parent is
blocked until child exec/exits.

## Where the fork_exit time actually lives

I re-profiled fork_exit after iter-1 to see if the blog-211
"80 % in VM fork" number had shifted.  Pulled `cntvct_el0` around
each phase inside `Process::fork` and around `duplicate_table`.

Post-iter-1:

| phase                               | ticks | µs  |
|-------------------------------------|------:|----:|
| `Process::fork` total               |  ~230 | 9.6 |
| ↳ `duplicate_table` (all levels)    |  ~140 | 5.8 |
| ↳ alloc + `Arc::new(Process {...})` |   ~65 | 2.7 |
| ↳ everything else                   |   ~25 | 1.0 |
| `Process::exit` (child)             |    ~6 | 0.3 |
| `PageTable::switch` (TLB flush)     |   ~4  | 0.2 |

Fork is **~10 µs**.  Of the 100 µs fork_exit, the remaining ~90 µs
is the child's on-CPU lifetime (syscall return, user `_exit(0)`,
syscall entry, kernel exit path, switch) plus the parent's wait4
sleep window.

Linux arm64 HVF does *the whole cycle* in ~16 µs.  The gap isn't
in our PT duplication (already 5.8 µs); it's in the child's
per-iteration overhead.

Specifically, the ARM64 trap path on every user exception saves &
restores 528 bytes of FP/NEON register state via `SAVE_FP_REGS` /
`RESTORE_FP_REGS`.  Four syscalls per fork_exit iteration (fork
entry/return in parent + exit entry/return in child) × 2 × 528 B
= 4 KiB of additional memory traffic per iter.  The kernel target
is `+neon,+fp-armv8`, so the handler *can* use FP internally;
disabling that target feature to skip FP save would be a real
surgery (every potential SIMD memcpy in deps has to be audited).

Linux arm64 builds its kernel *without* FP, and stashes FP state
only on context switch.  Porting that model here is a week of
work, not a day.

## Other options I considered and ruled out

**ASID-tagged TLBs.**  Would let `PageTable::switch` skip `tlbi
vmalle1` on context switch.  Measured cost of the current flush:
~170 ns.  Two switches per iter = 340 ns.  Adding ASID management
(16-bit ASID space, round-robin rollover, rollover-induces-flush)
to gain 340 ns per iter isn't worth the complexity.

**Ghost-fork (`GHOST_FORK_ENABLED = true`).**  Flips fork() into
vfork semantics: parent blocks until child exec/exits.
158/159 contracts pass; `process.wait4_wnohang` fails because it
expects `wait4(WNOHANG)` to return 0 when the child is still
running, but with ghost-fork the parent doesn't return from fork()
until the child has already exited.  This is a legitimate POSIX
violation — fork() must be concurrent.  musl uses `SYS_clone`, not
vfork, so we can't assume vfork semantics.  Ghost-fork is
useful for kernel-controlled scenarios (posix_spawn, maybe) but
not for blanket fork.

**Process struct surgery.**  `Arc::new(Process { ... })` takes
2.7 µs for a struct with ~50 fields, many of them `Arc<SpinLock<…>>`
or `SpinLock<Vec<…>>` with their own heap allocs.  Shrinking this
is the kind of thing that pays off in the 5-10 % range but
requires touching a lot of process lifecycle code.  Deferring.

**Leaf-PT share_leaf_pt walk in-fork vs lazy.**  Iter-1 walks the
leaf PT once during fork (512 entries × ~5 ns each = 2.5 µs per
PT).  Could be made lazier by deferring the data-page refcount
bumping into `unshare_leaf_pt`, paid only when the PT is actually
unshared.  Saves work if PT is never unshared (exec-then-drop).
Net win depends on workload — fork_exit always unshares — so
probably neutral.

## What I learned about the earlier profile

Blog 211 said "80 % of fork is PT duplication."  That was *before*
iter-1 (~19 µs VM fork out of ~23 µs total Process::fork).  After
iter-1, duplicate_table is ~5.8 µs out of ~10 µs Process::fork.
So iter-1 did shift the ratio — VM fork is now ~58 % of fork, not
80 %.  But the overall fork_exit number didn't move meaningfully
because fork is only 10 % of fork_exit to begin with; everything
else is context switching, syscall trap overhead, Arc lifecycle.

Had I measured fork_exit's actual breakdown (fork vs everything
else) before starting iter-1, I'd have known that chasing PT
duplication was the wrong lever.  Blog 211's profile was scoped to
Process::fork alone, not the full fork_exit cycle — it answered the
question "where in fork is time spent" correctly, but the question
I should have asked was "where in fork_exit is time spent."

## What's next

None of the remaining ~90 µs of fork_exit lives in code I'm eager
to touch.  Concrete next steps if someone wanted to chase it:

1. **Disable FP/NEON on the arm64 kernel target**, move FP save to
   context switch only.  Estimated win: ~1-2 µs/iter.  Risk: dep
   audit + potential regressions.
2. **Pool-allocate `Process` structs**.  Reuse Arc-exited structs
   instead of going through global allocator.  Estimated win:
   1-2 µs/iter.
3. **Reduce number of syscalls in fork_exit's critical path.**
   Child does one syscall (`_exit`), parent does two (fork,
   wait4).  Not obviously reducible without changing semantics.

Given the ratios, I'm going to shelve the fork_exit arc at
iter-1 + the ~35 %-already-earned from blogs 207-212.  Opening
other fronts for the next session — the remaining slow bench
ratios (`socketpair`, `pipe`, `read_zero`, `mmap_fault`) are each
a 3-5× gap on their own and are likely to yield more than the
residual fork_exit hunt.
