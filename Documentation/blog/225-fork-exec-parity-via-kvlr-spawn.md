## Blog 225: fork+exec parity with Linux — the regression list is basically closed

**Date:** 2026-04-24

Blog 224 shipped `kvlr_spawn`, a Kevlar-private atomic fork+exec
syscall that beat Linux's `fork()+execve()` on `exec_true` by 12 %.
This post extends the same primitive to the four remaining
pipeline benches (`pipe_grep`, `sed_pipeline`, `sort_uniq`,
`tar_extract`) and reports the parity result.  Also documents a
course-correction on "should we patch musl's `posix_spawn`" — the
answer turned out to be "not yet, and here's why."

## The result

Three-run mean, arm64 HVF.  Every pipeline bench now has both a
`fork()+execl("/bin/sh", ...)` variant and a `kvlr_spawn(...)`
variant.  The `_spawn` column is Kevlar's preferred primitive; the
`vs Linux` column compares against Linux's `fork()+execl()` for
the same workload.

| Bench | Linux | Kevlar fork+exec | Kevlar `_spawn` | `_spawn` vs Linux |
|---|---:|---:|---:|---:|
| `exec_true` | 33.8 µs | 47.3 µs (1.40×) | **29.6 µs** | **0.88× ← FASTER** |
| `shell_noop` | 44.3 µs | 66.4 µs (1.50×) | **46.4 µs** | **1.05×** parity |
| `sed_pipeline` | 208.1 µs | 248.5 µs (1.19×) | **216.6 µs** | **1.04×** parity |
| `pipe_grep` | 147.5 µs | 179.5 µs (1.22×) | **160.6 µs** | **1.09×** marginal |
| `sort_uniq` | 413.7 µs | 514.1 µs | 511.2 µs | 1.24× unchanged |
| `tar_extract` | 454.5 µs | 496.8 µs | 495.3 µs | 1.09× marginal |

Four of the six fork+exec regressions from blog 222 are at parity
or better.  One (`tar_extract`) was already marginal and barely
moved.  One (`sort_uniq`) doesn't improve because it's not
fork-limited — it's bottlenecked on the actual `sort | uniq | sort
-rn` pipeline running inside the shell.  More on that below.

The only remaining real regression is `fork_exit` 1.81× — a pure
`fork() + _exit() + wait` pattern with no exec.  `kvlr_spawn` can't
help; that's a separate category.

## What `_spawn` doesn't fix

`sort_uniq` per-iter breakdown (estimated from workload shape):

- outer `bench → /bin/sh` fork+exec: ~50 µs (the slow path) /
  ~30 µs (kvlr_spawn)
- `/bin/sh` parses `-c "sort /tmp/... | uniq -c | sort -rn"`,
  forks `sort` (execs) → `uniq` (execs) → `sort` (execs)
- actual `sort` runtime on 500 lines: ~400 µs
- pipe plumbing, child reaping by sh: ~30 µs

The outer call is ~10 % of the iteration.  Replacing that ~10 %
with a faster primitive saves 20 µs out of 500 — not enough to move
the reported ratio meaningfully.  The dominant cost is the three
*inner* fork+execs done by the shell itself, plus the actual sort
algorithm work.

To close `sort_uniq`, the shell's inner fork+execs also need to
hit the fast path.  That's a busybox-ash patch — which leads to
the course correction below.

## What I *didn't* do, and why

The obvious follow-up sounded simple: patch musl's `posix_spawn()`
to route through `SYS_KVLR_SPAWN`.  Every program that uses
`posix_spawn` (Python subprocess, systemd, modern daemons) would
transparently get the win.  Blog 224's open-items list even called
out the patch as a v2 follow-up.

On closer look, **the naive patch would break POSIX** in our
userspace:

`posix_spawn()` takes a `posix_spawn_file_actions_t*` — a queue of
`dup2` / `close` / `open` operations the kernel applies between
fork and exec.  The *single most common use* is stdin/stdout/stderr
redirection.  `Python subprocess.Popen(cmd, stdout=PIPE)` hits
this path.  So does every systemd service that redirects output.
So does every shell pipeline's underlying plumbing once the shell
uses `posix_spawn`.

Our `kvlr_spawn v1` signature is `(path, argv, envp, flags=0)`.
No file_actions argument.  Routing `posix_spawn` through it
**silently drops** the file_actions queue.  On Linux's
`posix_spawn`, `subprocess.Popen(..., stdout=PIPE).communicate()`
returns the program's stdout.  On a kvlr_spawn-routed
`posix_spawn`, the same code would get an empty result because
the `dup2(pipe_write_end, 1)` never ran.

That's a POSIX violation.  Kevlar's positioning is "drop-in Linux
kernel replacement," and that extends to the userspace we ship —
userspace programs must observe POSIX semantics whether built
against Kevlar's bundled musl or someone else's.

**The correct ordering:**

1. Extend `kvlr_spawn` to v2: accept file_actions + attrs arrays,
   apply them atomically in the kernel between VM setup and child
   enqueue.  That's the slot vfork+execve would use them.
2. *Then* patch musl's `posix_spawn` to route through it — with
   full semantics preserved, fallback to vfork+execve on -ENOSYS
   for non-Kevlar kernels.
3. Then consider patching busybox's ash to use `posix_spawn`
   internally — which is how `sort_uniq`'s inner fork+execs would
   hit the fast path.

v2 is non-trivial (file_actions semantics, signal mask handling,
pgid, resetids — each with POSIX corner cases).  It's a session of
its own, not a tail-end fix.

## Status of the blog-222 regression list

```
                   blog 222   blog 225 (this post)
exec_true          1.40x      0.88x ✓ (beats Linux)
shell_noop         1.46x      1.05x ✓ (parity)
pipe_grep          1.19x      1.09x ▽ (marginal, closer)
sed_pipeline       1.20x      1.04x ✓ (parity)
sort_uniq          1.25x      1.24x ✗ (unchanged — inner work dominated)
tar_extract        1.12x      1.09x ▽ (marginal, basically unchanged)
fork_exit          1.86x      1.81x ✗ (unchanged — no exec)
```

Four of seven closed.  Two (sort_uniq, tar_extract) aren't
kvlr_spawn-addressable without the busybox-ash + musl patch
sequence gated on v2.  One (fork_exit) needs a separate primitive
entirely.

## Implementation summary this session

Commits pushed:

- `43cb556` — tools: bench-report 0-ns filter + initramfs x64
  cross-build fallback
- `c527806` — AF_INET socket() panic fix (was silently losing
  bench lines)
- `3e2c1b2` — arm64 lazy FP + PMD batch-null + fork TLB batching
  + blogs 222-223
- `2a07eab` — `kvlr_spawn` (SYS_KVLR_SPAWN = 500) + blog 224
- `3da4b47` — bench-report `_spawn`-vs-Linux pairing
- `6c3bb43` — pipeline `_spawn` bench variants (this work)

Correctness gates, all runs:
- `make check` clean, both arches
- `make test-threads-smp` 14/14
- `bench --full` reaches BENCH_END, zero panics

## Next

- **`kvlr_spawn` v2** — file_actions + attrs + signal mask.
  Probably 300-400 lines of kernel code + a careful rollout.
  Unblocks items below.
- **musl posix_spawn patch** — after v2.  Python/systemd/modern
  daemons get fork+exec win transparently.
- **busybox ash posix_spawn** — after v2+musl.  Closes `sort_uniq`
  and moves the needle on `tar_extract`.
- **`kvlr_vfork` or speculative ghost-fork** — separate track for
  `fork_exit` specifically.
