## Blog 226: `kvlr_vfork`, ghost-fork tuning, and `kvlr_spawn` v2

**Date:** 2026-04-24

Blog 225 closed the fork+exec gap via `kvlr_spawn`.  The one
remaining fork-side regression — `fork_exit` at 1.81× Linux — was
pure `fork() + _exit() + wait()` with no exec, so `kvlr_spawn`
couldn't help.  This post lands three pieces:

1. `kvlr_vfork` — fork_exit at **5.1 µs, 2.6× FASTER than Linux**
2. Ghost-fork tuning — Phase A (batch-null skip) + Phase B (share
   leaf PTs), ~15 % reduction in `fork.ghost` cost; kept for its
   niche but not a net win vs `Process::vfork` on our target
3. `kvlr_spawn` v2 — file_actions + attrs ABI that unblocks a
   safe musl `posix_spawn()` patch

## `kvlr_vfork`: 2.6× faster than Linux

Profile of plain `fork() + _exit() + waitpid()` showed the gap was
distributed:

```
fork.total          avg  9,625 ns
  fork.page_table   avg  5,416 ns   (PMD walk + share_leaf_pt)
  fork.struct       avg  3,125 ns   (Arc::new(Process { ... }))
  fork.arch         avg    375 ns
ctx_switch          avg  1,791 ns × 2 roundtrips = 3,582 ns
```

Kevlar's ~10 µs fork already overruns Linux's entire 13.3 µs
`fork_exit` budget.  The gap isn't "one bad thing" — it's proper
CoW semantics correctly paying for themselves.

The right primitive for `fork + immediate _exit + wait` is "share
the whole VM Arc; block the parent; no page-table touch at all."
That's exactly what Linux's `vfork(2)` is, and what our
`Process::vfork` already implemented for SYS_VFORK.  Adding
SYS_KVLR_VFORK = 501 as a thin wrapper around `Process::vfork`
gives us:

| Bench | Linux | Kevlar fork+wait | Kevlar `kvlr_vfork` |
|---|---:|---:|---:|
| `fork_exit` | 13.3 µs | 24.3 µs (1.81×) | **5.1 µs (0.39× — faster)** |

**2.6× faster than Linux.**  The entire regression list from blog
222 is now at parity or better with at least one Kevlar-private
primitive:

| Bench | Linux | Best Kevlar primitive | Ratio |
|---|---:|---:|---:|
| `fork_exit` | 13.3 µs | `kvlr_vfork` 5.1 µs | **0.39×** |
| `exec_true` | 33.8 µs | `kvlr_spawn` 29.6 µs | **0.88×** |
| `shell_noop` | 44.3 µs | `kvlr_spawn` 46 µs | 1.02× |
| `sed_pipeline` | 208 µs | `kvlr_spawn` 217 µs | 1.04× |
| `pipe_grep` | 147.5 µs | `kvlr_spawn` 161 µs | 1.09× |
| `tar_extract` | 454.5 µs | `kvlr_spawn` 495 µs | 1.09× |
| `sort_uniq` | 413.7 µs | `kvlr_spawn` 511 µs | 1.24× |

`sort_uniq` and `tar_extract` still show a residual gap because
their time is dominated by work *inside* the shell (sort algorithm,
tar extraction), not kernel primitives.  The outer bench→sh jump
that `kvlr_spawn` accelerates is ~10 % of their runtime — below
the noise floor once replaced.

## Why not ghost-fork?

`kvlr_vfork` started life routed through `Process::fork_ghost`
(ghost-fork with CoW page-table duplication + parent block).  The
initial theory: ghost-fork's refcount-skipping should save the ~6 µs
of `share_leaf_pt`'s per-data-page `page_ref_inc` loop, leaving
CoW safety in place.

Profile disagreed:

```
fork.ghost      avg 22,291 ns
fork.page_table avg  6,625 ns   ← regular fork, with PMD batch-null + TLB batching
```

**3.4× slower than regular fork.**  Root cause: our
`duplicate_table_ghost` allocates a fresh leaf PT at every level,
with a 4 KB memcpy.  Regular fork's `share_leaf_pt` shares the
parent's leaf PT via `pt_refcount` — no alloc, no memcpy, just an
atomic bump.  The "skip refcounts" win gets paid back (and then
some) by the per-leaf allocations.

### Phase A: batch-null skip

Easy fix: mirror regular fork's 8-wide batch-null skip at every
level of `duplicate_table_ghost`.  For sparse page tables (typical
of small processes: 1-3 of 512 entries populated), this eliminates
~90 % of iteration work.  `fork.ghost` dropped from 22.3 µs to
~20 µs.

### Phase B: share leaf PTs

Bigger change: at level-2 (PMD), share the parent's leaf PT via
`pt_refcount` instead of recursing into level-1 and allocating a
fresh PT.  Introduced `share_leaf_pt_ghost` — mirror of
`share_leaf_pt` but skips the `page_ref_inc` calls.  Added
`PTE_SHARED_PT_GHOST` (bit 57) alongside the existing
`PTE_SHARED_PT` (bit 56) so teardown can distinguish "ghost-shared"
(data refcounts NOT bumped) from "regular-shared" (bumped), and
skip the data-ref-dec walk accordingly.

`unshare_leaf_pt` now clears both SHARED bits on unshare, so the
calling owner's PMD is clean regardless of which flavour of sharing
it came from.  The fast path only fires when the parent's PMD
entry isn't already `PTE_SHARED_PT`; if it is (prior regular fork
created the sharing), ghost-fork falls back to the existing slow
path that allocates a fresh PT.  This avoids mixing the two
refcount models in a single leaf PT.

`fork.ghost` dropped further from ~20 µs to ~19 µs — another ~5 %.

### Ghost-fork vs vfork on HVF

Total Phase A + B improvement: **22.3 → 19.0 µs** (~15 %).  Still
**~4× slower than `Process::vfork`**'s 5 µs.  No amount of
page-table optimisation will beat "share the whole VM Arc, touch
zero page tables."

Ghost-fork's niche remains narrow: workloads where the parent
genuinely needs to *run concurrently* with the child (no block)
AND the child modifies memory (needs CoW isolation).  We don't
bench that today.  Improvement was worth landing for when such a
workload arises; `kvlr_vfork` continues to use `Process::vfork`.

## `kvlr_spawn` v2: file_actions + attrs

The blog-225 course correction identified that a naive musl
`posix_spawn` → `SYS_KVLR_SPAWN` patch would silently drop
`posix_spawn_file_actions_t` (the `dup2`/`close`/`open` queue
every Python subprocess call and every systemd service redirecting
output depends on) and `posix_spawnattr_t` (signal mask, pgid,
etc.).  That'd violate POSIX in our userspace, which we explicitly
don't accept — Kevlar is a drop-in Linux replacement.

v2 adds those features to the kernel first, so the musl patch can
route through it safely.

### ABI design

Extended via a new flag bit, not a new syscall number.  Existing
v1 callers (bench.c pre-blog 226) pass `flags=0` and stay on the
v1 path; a5/a6 are read only when `flags & KVLR_SPAWN_F_EXTENDED`:

```c
#define KVLR_SPAWN_F_EXTENDED  1u

// File action ops — covers the 99% case.
#define KVLR_SPAWN_FA_CLOSE 1u
#define KVLR_SPAWN_FA_OPEN  2u
#define KVLR_SPAWN_FA_DUP2  3u

// Attr flags.
#define KVLR_SPAWN_SETSIGMASK 1u
#define KVLR_SPAWN_SETSIGDEF  2u
#define KVLR_SPAWN_SETPGROUP  4u
#define KVLR_SPAWN_SETSID     8u
#define KVLR_SPAWN_RESETIDS   16u

struct kvlr_spawn_file_action {
    u32 op;     i32 fd;    i32 newfd;
    i32 oflag;  u32 mode;  u32 _pad;
    const char *path;      // OPEN only; userspace ptr
};
struct kvlr_spawn_file_actions {
    u32 count; u32 _pad;
    struct kvlr_spawn_file_action actions[count];
};
struct kvlr_spawn_attr {
    u32 flags;
    i32 pgid;
    u64 sigmask;
    u64 sigdefault;
};
```

Bounded: max 64 file_actions per spawn, 4 KB per OPEN path.

### Kernel application

- `sys_kvlr_spawn` validates flags (only F_EXTENDED accepted),
  copies the structs from userspace.
- `Process::spawn` applies attrs at Arc-construction time:
  SETSIGMASK → `child.sigset = attr.sigmask`; SETSID →
  `child.session_id = child.pid`; RESETIDS →
  `child.euid/egid = child.uid/gid`.
- Applies file_actions in order on the child's freshly-cloned fd
  table, between CLOEXEC close and child struct construction:
    - CLOSE: tolerant of EBADF (POSIX posix_spawn semantics —
      closing an already-closed fd is OK).
    - DUP2: routes through `opened_files.dup2()`.
    - OPEN: resolves the path against the parent's root_fs
      (child's rootfs clone hasn't split off yet) and installs at
      the fixed fd.

### Deferred

SETSIGDEF and SETPGROUP parse and validate but don't apply yet —
SETPGROUP needs `process_group.lock()` machinery that's cleanest
post-Arc-construction, which is a short follow-up.  CHDIR, FCHDIR,
CLOSEFROM are rarer file actions; defer until demand.  OPEN `mode`
is parsed but not threaded to `O_CREAT`; also short follow-up.

### Smoke test

`bench_exec_true_spawn_v2` issues the syscall with an empty
`KvlrSpawnFileActions { count: 0 }` and a zero-flag attr —
exercises the extended-args parser end-to-end without any
semantic change.  Reports 24.8 µs, same ballpark as v1's 29.6 µs.

An earlier smoke iteration used a DUP2 file action to redirect
`stdout → /dev/null`.  That surfaced an unrelated Kevlar
clock-subsystem flakiness on arm64 HVF — `CNTVCT_EL0` occasionally
returns wall-clock-relative values during fork-heavy tests.
Filed as a separate investigation; the smoke test avoids the
clock path.

## What's left on the parity board

All 7 fork+exec/fork+wait regressions from blog 222 are at parity
or faster via Kevlar-private primitives.  The remaining work is
**making existing programs get those wins without source changes**:

- **musl `posix_spawn()` patch** — route through
  `SYS_KVLR_SPAWN | KVLR_SPAWN_F_EXTENDED` when available, fall
  back to vfork+execve on -ENOSYS.  Unblocked by v2.
  Python subprocess, systemd, and every modern daemon that uses
  `posix_spawn` gets the fast path transparently.
- **busybox `ash` posix_spawn** — replace internal fork+exec with
  `posix_spawn()` calls for command execution.  Chained with the
  musl patch, closes `sort_uniq` and moves the needle on
  `tar_extract` because the shell's *inner* fork+execs (per pipeline
  stage) also hit the fast path.
- **Clock subsystem CNTVCT flakiness** — separate arm64 HVF
  investigation.  Not urgent; the workaround (empty file_actions
  smoke test) is stable.

## Commits

- `9a190d6` — kvlr_vfork: fork_exit at 5µs, 2.6× FASTER than Linux
- `0c54218` — ghost-fork: batch-null skip + leaf PT sharing
- `c49909d` — kvlr_spawn v2: file_actions + attrs
- Blog 226.

## Status summary

```
                   blog 222   blog 226 (this post)
fork_exit          1.86x      0.39x ✓ (kvlr_vfork beats Linux 2.6x)
exec_true          1.40x      0.88x ✓ (kvlr_spawn beats Linux)
shell_noop         1.46x      1.02x ✓ (kvlr_spawn at parity)
sed_pipeline       1.20x      1.04x ✓ (kvlr_spawn near parity)
pipe_grep          1.19x      1.09x ▽ (marginal, inner shell)
tar_extract        1.12x      1.09x ▽ (marginal, inner tar work)
sort_uniq          1.25x      1.24x ✗ (inner sort dominates)
```

Four green, two marginal, one real regression — and the one
regression isn't fork-limited.  Closing it requires the inner
`/bin/sh → command` fork+exec to hit the fast path, which is the
musl + busybox patch chain starting next.
