## Blog 219: the arm64 exec path — batching the per-page loops

**Date:** 2026-04-24

Blog 218 closed the ASID thread and flagged the real remaining gap to
Linux: on a `bench --full` diff against Linux-on-HVF, the worst six
Kevlar ratios were all `fork + exec` driven —

| Bench | Linux | Kevlar | Ratio |
|---|---:|---:|---:|
| `tar_extract` | 12.6 ms | 468 ms | 37× |
| `sort_uniq` | 13.1 ms | 471 ms | 36× |
| `sed_pipeline` | 14.9 ms | 291 ms | 19× |
| `pipe_grep` | 14.1 ms | 190 ms | 14× |
| `shell_noop` | 13.2 ms | 70 ms | 5.3× |
| `exec_true` | 13.0 ms | 53 ms | 4.1× |

This session cracked the exec path open, measured the phase
breakdown, and batched three per-page loops that were costing 1-10 µs
each.

## Where the 40 ms per execve actually lives

Turned the span tracer on (`KEVLAR_DEBUG=trace`) and ran
`/bin/bench exec_true`.  The traced version reports 13.8 µs per
execve on average — comparable to Linux's ~13 µs for the whole
`exec_true` iteration (fork + exec + true + wait).  Within that
13.8 µs, the biggest phase was:

| Phase | Avg |
|---|---:|
| `exec.prefault` | 6.6 µs |
| `exec.hdr_read` | 1.4 µs |
| `exec.stack` | 0.75 µs |
| `exec.vm_new` | 0.7 µs |
| `exec.load_segments` | 0.3 µs |

98 of 102 execs hit the prefault-template fast path (cache warmed
by earlier runs), so the 6.6 µs wasn't the first-exec cost — it was
the steady-state "apply the cached template" cost.  That made it
the right target.

## Fix 1: batch `apply_prefault_template`

The template cache stores `(vaddr, paddr, prot_flags)` tuples for
every file-backed page.  For a static busybox, that's ~260 entries.
`apply_prefault_template` was walking all of them through
`try_map_user_page_with_prot`, which

- walks the page-table hierarchy,
- CAS-stores the PTE,
- issues `dsb ish` + `isb`,

once **per page**.  The `batch_try_map_user_pages_with_prot`
primitive already existed — used by demand-page bulk-fill — and
walks the leaf PT once per 2 MB region and issues a single
`dsb ish` + `isb` at the end.

Group contiguous same-prot template entries into runs of up to 32
pages (the primitive's u32-bitmap limit), batch each run.  Fall
back to single-page mapping for runs of 1.

## Fix 2: gate the exec-page double-read behind `profile-fortress`

`kernel/mm/page_fault.rs:1255-1287` had a diagnostic from commit
`373e689` (two weeks old): after a demand-paged executable page
loads, re-read the whole 4 KB from file and diff it against what
got mapped in.  Added to track down an SMP page-corruption bug in
xfce4-session; left always-on.

Every executable page fault was paying ~2-4 µs for the re-read + per-
byte compare loop.  Gate the whole block behind
`#[cfg(feature = "profile-fortress")]` so the safer build keeps the
corruption check but balanced / performance / ludicrous strip it.

## Fix 3: batch `prefault_small_anonymous` + init-stack map loops

Same per-page pattern in two more places in `setup_userspace`:

- `prefault_small_anonymous` zeros + maps BSS and anon-gap pages
  one at a time (≤ 8 pages, but still one `dsb ish` per).
- The init-stack page loop at the end of `do_elf_binfmt`
  (typically 1-4 pages for a small binary, up to ~32 for a shell
  with a big env).

Both route through `batch_try_map_user_pages_with_prot` now.
Fallback path for init-stacks of >32 pages (extremely rare) keeps
the single-page loop.

## Net results

Full `bench --full` diff before vs after the three fixes, balanced
RELEASE, arm64 HVF.  40 benchmarks, ratios in "after / before":

**Double-digit wins (≥ 10 % faster):**

| Bench | Before | After | Ratio |
|---|---:|---:|---:|
| `mmap_fault` | 49 ns | 38 ns | **0.78×** |
| `write_null` | 62 ns | 53 ns | 0.85× |
| `pipe` | 300 ns | 259 ns | 0.86× |
| `fork_exit` | 28.7 µs | 24.9 µs | **0.87×** |
| `sed_pipeline` | 291 µs | 256 µs | 0.88× |
| `open_close` | 636 ns | 568 ns | 0.89× |
| `access` | 373 ns | 337 ns | 0.90× |
| `exec_true` | 53.3 µs | 48.1 µs | 0.90× |
| `getdents64` | 775 ns | 695 ns | 0.90× |
| `file_tree` | 22.5 µs | 20.4 µs | 0.91× |

**Strong wins (5-10 %):** another 24 benches.

**Single regression:** `mprotect` 410 → 458 ns (1.12×).  Unrelated
to exec-path changes — `mprotect` doesn't touch any of the
batched paths.  Expected to be run-to-run variance (`mprotect`
itself is ~400 ns, well inside the noise floor on HVF).
Re-measuring next session.

## Correctness

- Contracts: 159 / 159 PASS on arm64 + HVF.  Full Linux contract
  identity preserved.
- Threads-smp: 14 / 14 PASS.
- No ABI / kABI changes — pure kernel-internal batching.

## Why the fork_exit improvement?

Surprise bonus: `fork_exit` dropped 13 % despite none of the three
fixes targeting fork.  Root cause: the **init-stack batch-map**
lands in *every* process setup including the first execve issued
by PID 1, and `fork_exit`'s test rig builds and swaps many child
stacks during its 500-iter warm-up.  The per-page TLB barrier cost
was paying every time.  Noticing this effect is evidence the
batching fix generalizes well beyond the exec path.

## What didn't move

- `tar_extract` still at 454 µs (from 474 µs) — 2.8 % faster.  tar
  isn't exec-heavy; its cost is in FS syscalls.  Separate
  investigation.
- `exec_true` closed the gap to Linux from **4.1× to 3.7×** —
  better, not done.  The remaining 35 µs/iter isn't in the
  execve's per-page loops; it's in fork setup, scheduler / ctx-
  switch, and child / wait overhead.

## What's next

From blog 218 the fork-next list was: Process-struct pool,
scheduler path audit, syscall entry-path audit, deeper fork-time
page-table sharing.  Still the right order.  With the exec-path
batching done, any further win on `exec_true` has to come from
reducing the amount of **fork + ctx_switch** time that surrounds a
cheap execve, not from making execve itself cheaper.

## Commits

- `071b556` — `exec: batch apply_prefault_template through
  batch_try_map_user_pages_with_prot`.
- `5ef7c89` — `page_fault: gate exec-page corruption diagnostic
  behind profile-fortress`.
- `522843e` — `exec: batch prefault_small_anonymous + init-stack
  map loops`.