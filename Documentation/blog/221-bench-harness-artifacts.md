## Blog 221: the 35× tar_extract gap was a harness artifact

**Date:** 2026-04-24

Blog 220 closed with "tar_extract still at 35× Linux — separate
investigation."  This post does that investigation, finds that the
35× and 32× and 19× ratios were all measuring different things on
the two kernels, fixes the harness so both sides run the same
workload, and re-reports the real Kevlar-vs-Linux picture with every
per-bench gap under 2×.

Also lands three smaller pieces from the "exec + fork" punchlist:
Fix 1 (Process pool — deferred), Fix 2 (u64 batch primitive), and
Fix 3 (page-aligned initramfs + `DIRECT_MAP`).

## The artifact

`bench_tar_extract` and `bench_sort_uniq` call:

```c
execl("/bin/sh", "sh", "-c",
      "rm -rf ...; mkdir ...; tar xf /tmp/bench.tar -C ...", NULL);
```

and

```c
execl("/bin/sh", "sh", "-c", "sort /tmp/benchfile | uniq", NULL);
```

**Kevlar's** test initramfs has `/bin/sh` (symlinked to `busybox`).
Execve succeeds, busybox ash starts up, parses the `-c` string,
fork/execs the sub-commands.  On the first pass `rm`, `tar`, `sort`,
`uniq` **weren't in our initramfs** either — not even as busybox
symlinks — so ash printed "rm: not found" and exited.  Per iter cost:
busybox ash startup + 3 × PATH lookup failures.  That's ~440 µs.

**Linux's** test initramfs (`tools/linux-on-hvf/` harness) contained
only the bench binary — no `/bin/sh`, no busybox, nothing.  So
`execl("/bin/sh", ...)` failed immediately with ENOENT, child
`_exit(127)`'d, parent's waitpid returned.  Per iter: one fork, one
fast-fail execve, one reap.  ~13 µs.

We were timing **busybox ash startup on Kevlar** vs **execve-ENOENT
on Linux**.  The "35×" was real, but it was the ratio of
`busybox_ash_startup / execve_fail` — a property of the test
harness, not of either kernel.

## Fixing the harness

Two changes:

1. **`tools/build-initramfs.py`**: when the host (arm64 busybox on
   Mac) can't enumerate applets via `busybox --list-full`, we fall
   back to a hardcoded list.  Added `tar`, `rm`, `cp`, `mv`, `sort`,
   `uniq`, `find`, `chmod`, `chown`, `ln`, `touch`, `dd`, `stat`,
   `df`, `du`, `basename`, `dirname`, `xargs` — all applets busybox
   already carries and the shell benches need.

2. **`tools/linux-on-hvf/Makefile`**: boot Linux with the *exact same*
   `/tmp/kevlar-rootfs.cpio.gz` Kevlar uses, passed via
   `rdinit=/bin/bench`.  Symmetric workload, symmetric comparison.

Then rerun `bench --full` on both.

## The real picture

Both kernels running the *same* initramfs (busybox + symlinks), arm64
HVF on Apple Silicon:

| Bench | Kevlar | Linux | Ratio |
|---|---:|---:|---:|
| `fork_exit` | 25 µs | 13 µs | 1.86× |
| `exec_true` | 48 µs | 31 µs | 1.52× |
| `shell_noop` | 68 µs | 43 µs | 1.59× |
| `pipe_grep` | 176 µs | 148 µs | 1.19× |
| `sed_pipeline` | 257 µs | 205 µs | 1.25× |
| `sort_uniq` | 539 µs | 401 µs | **1.34×** |
| `tar_extract` | 514 µs | 458 µs | **1.12×** |
| `file_tree` | 19 µs | 30 µs | **0.62× — Kevlar faster** |

Every gap under 2×.  `file_tree` actually wins against Linux.

`sort_uniq` and `tar_extract`, previously "the two worst outliers," are
now in the 1.1-1.4× band — ordinary syscall / exec overhead, nothing
pathological.

## Fix 2: widen `batch_try_map_user_pages_with_prot` to u64

The primitive returned a `u32` bitmap → capped batches at 32 pages.
`apply_prefault_template` for busybox (~260 entries) was running 9
batches; widening to `u64` drops that to 5 — ~45 % fewer leaf-PT
traversals.

In practice, per-fix perf impact lands under bench noise (fork_exit
varied 23-25 µs across three runs; no visible delta from the widen).
Committed anyway for the cleaner primitive and the headroom for
future callers with bigger batches.

## Fix 3: page-aligned initramfs + enable `DIRECT_MAP`

Covered in its own commit; full writeup is the commit body, summary
there.  TL;DR: `InitramFs::new_with_align` relocates every regular
file's bytes into page-aligned kernel pages marked with
`PAGE_REF_KERNEL_IMAGE` sentinel; flip `DIRECT_MAP_ENABLED = true`;
demand-pager stops doing alloc + copy, fork's `share_leaf_pt`
refcount bumps short-circuit on direct-mapped paddrs.  Small wins
across the suite, biggest `mprotect` -5.6 % and `mmap_fault` -4.4 %.

## Fix 1: Process pool (deferred)

Profile of `fork.struct` (1.6 µs) broke down as:

```
fork.pre_arcs   0.71 µs  (3x Arc<SpinLock<T>> allocs)
fork.arc_alloc  0.88 µs  (big Arc::new(Process {..}) — mostly
                          field writes, not alloc)
```

A pool saves only the ~200-400 ns of raw allocation cost, not the
field-write time.  Not worth the custom-allocator plumbing.  Left
in the punchlist but deprioritized.

## What's left

Under a 2× gap, **no single Kevlar lever is obviously wrong**; the
remaining overhead is ordinary per-syscall / per-trap / per-
context-switch cost spread across hundreds of call sites.  Closing
the last 30-50 % would require touching the syscall entry path,
scheduler, and trap handlers line by line.

That's the next session's shape if we want to keep chasing fork/
exec specifically.  For XFCE specifically, we should also focus on
heavy-syscall workloads (icon loading, config reads) where small
per-syscall wins add up — file_tree already winning suggests our
fs path is in decent shape there.

## Commits this session

Prior session: `071b556`, `5ef7c89`, `522843e`, `7505f6a`,
`4a81c95`, `971f113`, `0c919a6`.

This session:
- `c0ea02a` — initramfs: page-align file data + enable
  DIRECT_MAP_ENABLED (Fix 3).
- `ee92c18` — paging: widen batch_try_map_user_pages_with_prot to
  u64 mask (Fix 2).
- `fe584c7` — bench: symmetric Linux-on-HVF harness + real busybox
  tools (tar_extract / sort_uniq investigation).
- `(this commit)` — blog 221.