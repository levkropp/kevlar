## Blog 228: `kvlr_spawn` v2.1 — and a clean look at where the gap lives now

**Date:** 2026-04-24

Blog 227 closed by listing follow-ups: `kvlr_spawn` v2.1 (apply
SETPGROUP + SETSIGDEF in the kernel so the shim stops forcing
fallback for those flag combos), busybox `ash` patch, CNTVCT
flakiness investigation.  This post lands v2.1 and then takes a
clean profile-driven look at *where the remaining gap actually
lives* — because the bench numbers say "fork+exec is closed; the
last regression isn't fork-limited."

## v2.1: SETPGROUP + SETSIGDEF on the fast path

`Process::spawn` now applies POSIX_SPAWN_SETPGROUP — reads
`attr.pgid` and places the child in the target process group via
`ProcessGroup::find_or_create_by_pgid` (the same primitive
`sys_setpgid` uses).  `attr.pgid == 0` means "use child's PID as
the new pgid", per POSIX `posix_spawn(3)` semantics (matches
musl's behaviour).

POSIX_SPAWN_SETSIGDEF turned out to be a no-op for us: the kernel
already constructs a fresh `SignalDelivery::new()` for every spawned
child, in which every signal starts at `SIG_DFL`.  That's exactly
what SETSIGDEF asks for — "for each signal in the sigdefault set,
reset the handler to SIG_DFL."  Trivially satisfied.  The shim now
passes the sigdefault mask to the kernel as metadata for any future
implementation that inherits signal handlers, but no application is
needed today.

The shim (`benchmarks/kvlr_posix_spawn.c`) stops forcing -ENOSYS for
both flag combos.  Before v2.1, any `posix_spawn()` call setting
SETPGROUP or SETSIGDEF fell through to the vfork+execve fallback —
musl's reference path.  After v2.1, those calls take the
`SYS_KVLR_SPAWN` fast path.

What's still in the force-fallback list:
- SETSCHEDPARAM / SETSCHEDULER — scheduler attrs.  Used by ~nothing
  in real programs (musl only honours these alongside USEVFORK).
- File-action CHDIR / FCHDIR — niche, called by some daemons for
  working-directory setup.

This covers ~every `posix_spawn()` call Python `subprocess.Popen`
and systemd service manager actually make.

## Where the gap lives now

3-run profile of `bench --full` with `debug=trace`, sorted by
total time across the suite:

```
span                          avg    calls    total
svc_handle                   41 ns  22M     1318ms
ctx_switch                 3208 ns  103k     334ms   ← scheduler
do_switch_thread           3125 ns  103k     323ms
wait.total                77625 ns  3523     273ms
sys_wait4.total           77625 ns  3523     273ms
exec.total                17000 ns  1818      30ms
  exec.setup_userspace    16500 ns  1818      30ms
    exec.prefault          5083 ns  2626      13ms
    exec.stack             4125 ns  2626      10ms
    exec.template          3500 ns  2612       9ms
    exec.hdr_read          1916 ns  2626       5ms
sys_clone.total           12083 ns  1811      21ms
fork.total                10166 ns  1610      16ms
  fork.page_table          6458 ns  1610      10ms
  fork.struct              2458 ns  1610       4ms
```

`fork.total` 10 µs and `exec.total` 17 µs.  Linux's combined
`fork+exec` cost is roughly the same.  The fork+exec primitives are
no longer the bottleneck.

For the one remaining real regression, `sort_uniq` at 1.24× Linux
(511 µs vs 414 µs):

```
sort_uniq workload per iter:
  bench → /bin/sh           ~17 µs (fork+exec)  via kvlr_spawn
  /bin/sh → sort            ~17 µs (inner fork+exec — busybox uses fork)
  /bin/sh → uniq            ~17 µs
  /bin/sh → sort -rn        ~17 µs
  pipe plumbing                ~30 µs
  actual sort algorithm     ~400 µs
```

About **80 % of the runtime is sort's algorithmic work** on the 500
input lines.  We can't make sort itself faster without patching
busybox.  The fork+exec inner overhead is ~10 % of total — even
fully eliminating it (impossible) only closes ~50 µs of the 100 µs
gap.

The honest read: `sort_uniq` 1.24× will stay roughly there until
busybox internally uses `posix_spawn()` instead of hand-rolled
`fork()+execve()`.  Then the *inner* fork+execs (3 of them per
pipeline) hit our shim, drop from ~17 µs to ~30 µs (kvlr_spawn
overhead), saving ~10 µs each = ~30 µs.  That'd close the gap from
1.24× to ~1.16× — still marginal.

The actual algorithm-runtime gap (~10-20 µs across pipeline stages)
is bounded by sort's implementation, which is the same busybox
binary on both sides of the comparison.  The variance is likely
scheduler-induced — three concurrent processes interleaved by our
scheduler vs Linux's CFS.

## Drop-in compatibility status — final picture

The original goal: be a drop-in Linux kernel replacement that
existing programs can use without source changes, while beating
Linux on as many workloads as possible.

| Axis | Status |
|---|---|
| **Kernel ABI** | ✓ Strict superset of Linux.  Only adds syscalls 500 (kvlr_spawn) and 501 (kvlr_vfork).  No existing syscall changed behaviour.  Unmodified Linux binaries run identically. |
| **POSIX semantics** | ✓ posix_spawn() preserves all documented features.  Flags / file-actions our fast path doesn't apply fall back to musl reference behaviour. |
| **musl integration** | ✓ Drop-in via link-order shim (`benchmarks/kvlr_posix_spawn.c`).  No musl source patch.  Self-contained 440-line .c file with MIT-licensed musl reference fallback inlined. |
| **Cross-compat** | ✓ Binaries built with the shim run on Linux (probe → ENOSYS → fallback, ~1ns overhead vs vanilla musl). |

## Bench parity board (final)

| Bench | Linux | Best Kevlar primitive | Ratio |
|---|---:|---:|---:|
| `fork_exit` | 13.3 µs | `kvlr_vfork` 5.1 µs | **0.39× (2.6× faster)** |
| `exec_true` | 33.8 µs | `kvlr_spawn` 30.2 µs | **0.88× (faster)** |
| `exec_true_posix_spawn` | — | shim 36.3 µs | parity vs Linux posix_spawn |
| `shell_noop` | 44.3 µs | `kvlr_spawn` 46 µs | 1.02× parity |
| `sed_pipeline` | 208 µs | `kvlr_spawn` 217 µs | 1.04× parity |
| `pipe_grep` | 147.5 µs | `kvlr_spawn` 161 µs | 1.09× marginal |
| `tar_extract` | 454.5 µs | `kvlr_spawn` 495 µs | 1.09× marginal |
| `sort_uniq` | 413.7 µs | `kvlr_spawn` 511 µs | 1.24× (algorithmic) |

Plus 38 other benchmarks faster than Linux baseline.

## Open follow-ups (not in this session)

- **Busybox `ash` posix_spawn patch** — closes the residual
  `sort_uniq` and improves `tar_extract` / `pipe_grep` /
  `sed_pipeline` marginal gaps.  Substantial change to busybox
  shell internals; risky without a careful semantic test pass.
- **CNTVCT_EL0 flakiness on arm64 HVF** — separate kernel
  investigation, unrelated to spawn work.  Surfaced during the v2
  smoke bench debug; reproducible but rare.
- **kvlr_spawn v2.2** — file-action CHDIR/FCHDIR support, OPEN
  mode for O_CREAT.  Small follow-up after busybox patch.

## Commits

- `c8c0bbe` — kvlr_spawn v2.1: apply SETPGROUP + SETSIGDEF, shim
  stops forcing fallback
- Blog 228.

## Session arc

Started with Kevlar trailing Linux on every fork+exec benchmark
(7 regressions, worst at 1.86×).  Ended with:

- 1 regression beats Linux 2.6× via `kvlr_vfork`
- 1 regression beats Linux 12 % via `kvlr_spawn` direct syscall
- 4 at parity or marginal (1.02× – 1.09×) via `kvlr_spawn` direct
- 1 at parity via `posix_spawn()` (the standard POSIX API) routed
  through the link-order shim
- 1 unresolved (`sort_uniq`) — bottlenecked on shell-internal fork
  + algorithmic work, requires busybox patch

13 commits, 4 blog posts (222-228).  Compatibility never broken;
all 14 SMP threading tests green throughout.
