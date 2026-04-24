# Fix 1 deferred: Process-struct allocation pool

**Date:** 2026-04-24 (session after blog 221)
**Status:** **Deferred indefinitely.**  Low ROI, high complexity.

## The lever

`fork.struct` on arm64 HVF measures 1.58 µs per fork, broken down as:

```
fork.pre_arcs   0.71 µs    3× Arc<SpinLock<T>> allocations
                           (opened_files, root_fs, signals)
fork.arc_alloc  0.88 µs    Arc::new(Process { ... })
```

The original "pool" idea was to reuse the 2.5 KB `Arc<Process>` heap
allocation across forks by keeping a per-CPU stack of recyclable
buffers.

## Why the ROI is under 1 µs

The 0.88 µs `Arc::new(Process)` splits roughly as:

- **~700 ns** — writing ~50 fields (atomics, spinlocks, refcells,
  boxed trivia).  This is CPU work, not allocation, and doesn't
  change with a pool.
- **~100 ns** — memcpy'ing the stack-built struct into the heap
  `ArcInner`.
- **~80 ns** — buddy_alloc heap allocation itself.

A pool can only save the ~80 ns of heap alloc.  On the fork_exit
workload (1 fork per iter), that's 80 ns / 24 700 ns total iter =
**0.3 % of fork_exit**.  Well under bench noise.

## The three inner Arcs

`opened_files`, `root_fs`, `signals` are wrapped in
`Arc<SpinLock<T>>` because threading (CLONE_FILES, CLONE_FS,
CLONE_SIGHAND) can share them between tasks in the same thread
group.  Regular fork creates a fresh `Arc<SpinLock<T>>` with cloned
contents per field — 3 Arc allocations, ~230 ns each, ~710 ns total.

Eliminating these would require:

- Inline the `SpinLock<OpenedFileTable>` etc. in `Process` for the
  single-threaded case.
- On thread clone, promote the inline field to an `Arc<SpinLock<T>>`
  and rewrite every access site to use a dual-mode wrapper.

That's a sweeping refactor of the file / signal / rootfs subsystems,
not a 50-line fix.  Potential win: ~700 ns per fork, which is 3 % of
fork_exit.  Still small relative to the remaining 12 µs gap to
Linux.

## Why `allocator_api` didn't help either

`Arc::new_in(x, CustomAlloc)` lets you inject a custom allocator per
Arc, which is the clean way to pool.  But:

- `allocator_api` is nightly-only (OK — we are on nightly).
- Every call-site taking `Arc<Process>` has to turn into
  `Arc<Process, PoolAlloc>` — type signature change throughout the
  kernel and many services.  ~200+ sites.
- The saved 80 ns doesn't justify 200+ type-signature changes and
  churn in code review.

## What I looked at

- **Shrinking the Process struct.**  Blog 216 already compressed it
  from 3 656 B to 2 536 B.  Every surviving field is actively
  read by at least one subsystem.  No dead fields to remove.
- **Lazy-boxing rlimits / cached_utsname.**  Both are read on
  common syscalls (getrlimit during shell startup, uname on every
  process init).  Lazy-init would cost more than it saves.
- **Merging the three inner Arcs into one `ProcessInner`.**
  Requires CLONE_FILES / CLONE_SIGHAND / CLONE_FS to always share
  together, which Linux doesn't — so not POSIX-compatible.

## Conclusion

`fork.struct` at 1.58 µs is already close to the floor for a struct
of this size and field count.  The path to meaningful fork speedup
is **not** struct allocation — it's per-syscall, per-trap, per-
context-switch overhead further up the stack.

Closing this ticket.  Revisit only if:
- A future workload measures `fork.struct` consuming > 5 % of some
  benchmark we care about; or
- We need Process-struct pooling for a different reason (e.g.,
  NUMA locality, deterministic timing) that bundles the savings
  with other wins.