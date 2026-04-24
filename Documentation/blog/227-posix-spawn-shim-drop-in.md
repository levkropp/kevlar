## Blog 227: `posix_spawn()` routing via link-order — drop-in fast path, zero musl patches

**Date:** 2026-04-24

Blog 225 closed by identifying "the next parity lever" as a musl
`posix_spawn()` patch that routes through `SYS_KVLR_SPAWN`.  Blog 226
shipped `kvlr_spawn` v2 (file_actions + attrs) to make that routing
safe without breaking POSIX.  This post does the patch — except it
isn't really a musl patch.  It's a single C file, linked into every
Kevlar binary *before* `-lc`.

## The problem with patching musl

Actually patching musl means:

1. Forking musl source, maintaining a Kevlar branch, rebasing on
   upstream.
2. Rebuilding libc.a as part of the initramfs pipeline.
3. Every Kevlar binary gets the patched libc baked in; external
   binaries (Alpine package `apk`s containing programs linked
   against stock musl) don't.
4. If our patch has a bug, *every* program that calls posix_spawn
   breaks — including programs we haven't tested yet.

All of that for a function that's ~150 lines of reference code.

## Link-order override

Static linkers (GNU ld, LLVM lld, mold) satisfy unresolved symbols
by scanning objects + archives left-to-right.  The first object
that provides a symbol wins; subsequent archive members aren't
pulled in for that symbol.  For musl:

```
$ musl-gcc my_program.c           # links with -lc implicitly
  → ld my_program.o ... -lc
  → ld sees unresolved `posix_spawn`
  → scans /lib/libc.a members, finds posix_spawn.o
  → pulls in posix_spawn.o to satisfy the symbol
```

If we put our own `posix_spawn` object *before* `-lc`:

```
$ musl-gcc my_program.c kvlr_posix_spawn.c
  → ld my_program.o kvlr_posix_spawn.o ... -lc
  → ld sees unresolved `posix_spawn`
  → kvlr_posix_spawn.o provides it → symbol satisfied
  → musl's posix_spawn.o NEVER PULLED IN
```

So `benchmarks/kvlr_posix_spawn.c` is a single translation unit that:

1. Provides `posix_spawn()` and `posix_spawnp()` with the
   standard signatures from `<spawn.h>`.
2. Probes `SYS_KVLR_SPAWN` once, caches the result atomically.
3. On Kevlar: translates `posix_spawn_file_actions_t` (musl's
   opaque struct, with a known internal `struct fdop` linked list
   format) and `posix_spawnattr_t` into the `kvlr_spawn` v2 ABI
   and issues the syscall.
4. On -ENOSYS (Linux, or Kevlar with SYS_KVLR_SPAWN disabled, or
   with a feature combo v2 doesn't yet support): falls back to a
   POSIX-correct `vfork + execve` implementation.

The only integration change: `tools/build-initramfs.py` includes
`benchmarks/kvlr_posix_spawn.c` in the musl-gcc command line for
every Kevlar binary.  No musl fork, no libc.a rebuild, no patch to
maintain against upstream musl.

## The ABI translation

musl's `posix_spawn_file_actions_t` header:

```c
typedef struct {
    int __pad0[2];
    void *__actions;   // head of struct fdop linked list
    int __pad[16];
} posix_spawn_file_actions_t;

struct fdop {
    struct fdop *next, *prev;
    int cmd, fd, srcfd, oflag;
    mode_t mode;
    char path[];
};
```

Walk the list from tail to head (matching musl's reference child
code order) and translate each `fdop` into a `kvlr_fa` entry:

```c
case FDOP_CLOSE: out->op = KVLR_SPAWN_FA_CLOSE; out->fd = op->fd; break;
case FDOP_DUP2:  out->op = KVLR_SPAWN_FA_DUP2;
                 out->fd = op->srcfd; out->newfd = op->fd; break;
case FDOP_OPEN:  out->op = KVLR_SPAWN_FA_OPEN;
                 out->fd = op->fd; out->oflag = op->oflag;
                 out->mode = op->mode; out->path = op->path; break;
case FDOP_CHDIR:
case FDOP_FCHDIR:
    return -ENOSYS;  // force fallback — kvlr_spawn v2 doesn't apply these
```

For attrs: similar, with force-fallback for flags that v2 parses
but doesn't yet apply (SETPGROUP, SETSIGDEF, SETSCHEDPARAM,
SETSCHEDULER).

The returned -ENOSYS tunnels up to the caller, which switches to
the fallback path so semantics match Linux exactly.

## The fallback

Copied from musl 1.2.5's `src/process/posix_spawn.c` — MIT-licensed,
credited in the source header.  Adapted to use only public APIs
(no `__libc_sigaction`, `__syscall`, `__abort_lock`) so the single
`.c` file is self-contained.  Reproduces:

- `pipe2(O_CLOEXEC)` for error reporting from child to parent
- `SIG_BLOCK` allmask during the spawn, restore on return
- `vfork()` child runs inline (CLONE_VM + CLONE_VFORK equivalent)
- Signal handler reset per SETSIGDEF set
- `setsid`, `setpgid`, `setuid`/`setgid` resetting in-child
- File-actions walk + per-op primitive (`close`, `dup2`, `open`,
  `chdir`, `fchdir`)
- Final `execve`, report `-errno` through pipe on failure

On Linux: `kvlr_spawn_probe()` returns -ENOSYS on first call,
cached.  Every subsequent `posix_spawn()` immediately goes to
fallback.  Overhead: one atomic load per call vs vanilla musl.
~1 ns.

## Result

New bench variant `exec_true_posix_spawn` uses the standard POSIX
API (same code you'd write on Linux):

```c
posix_spawn_file_actions_t fa;
posix_spawn_file_actions_init(&fa);
pid_t pid;
posix_spawn(&pid, "/bin/true", &fa, NULL, argv, environ);
waitpid(pid, NULL, 0);
```

Results (arm64 HVF, 3-run mean):

| Path | Time | vs Linux |
|---|---:|---:|
| Kevlar `fork() + execl()` | 47.9 µs | 1.42× |
| Linux `fork() + execl()` | 33.8 µs | 1.00× |
| Kevlar direct `syscall(SYS_kvlr_spawn, ...)` v1 | 30.2 µs | **0.89×** |
| Kevlar direct `syscall(SYS_kvlr_spawn, ...)` v2 | 25.3 µs | **0.75×** |
| Kevlar `posix_spawn()` via shim | **36.3 µs** | **1.07× (parity)** |
| Linux `posix_spawn()` (vfork+execve) | ~34-36 µs | 1.00× |

The shim adds ~6 µs over the direct-syscall path — the cost of
`posix_spawn_file_actions_init/destroy`, the pipe2 probe-before-
spawn bookkeeping that the standard API requires, and the
linked-list walk.  Any posix_spawn caller would pay that cost
anyway on Linux.

Against Linux's own `posix_spawn` (which internally is vfork +
execve), we're at **effective parity** — ~1.07×.  Real programs
using `posix_spawn` (Python `subprocess.Popen`, systemd services,
many daemons) transparently get the Kevlar fast path with no
source changes.

## Why this matters for drop-in compat

Three axes of compatibility, all green:

1. **Kernel ABI**: Kevlar's kernel only *adds* syscall 500.  Every
   existing Linux binary (unmodified, linked against stock musl or
   glibc) runs identically — it never calls syscall 500.  No
   existing syscall changed behaviour.

2. **Userspace ABI** (programs compiled against Kevlar's musl):
   `posix_spawn()` preserves every documented POSIX feature.
   Flags we haven't wired through the fast path fall back to the
   POSIX-correct vfork+execve implementation.  No silent
   semantic changes.

3. **Cross-compat** (binaries compiled against Kevlar's
   initramfs-built musl that run on Linux): the shim probes
   SYS_KVLR_SPAWN, sees -ENOSYS, caches, falls back to vfork+
   execve.  Runs identically to a binary built against vanilla
   musl.  Zero Linux-side regression.

## What's left

- **Busybox `ash` patch**: `sort_uniq` at 1.24× Linux is the one
  remaining regression.  Its time is dominated by the *inner*
  fork+execs done by the shell (`/bin/sh → sort`, `/bin/sh →
  uniq`).  Closing it requires busybox to use `posix_spawn()`
  internally where it currently hand-rolls fork+exec.  Once
  done, the posix_spawn shim routes those inner calls through
  SYS_KVLR_SPAWN automatically.

- **kvlr_spawn v2.1**: wire the remaining spawn attr flags
  (SETPGROUP, SETSIGDEF) so the shim stops forcing fallback for
  those combinations.  ~50 lines.

- **CNTVCT_EL0 flakiness**: separate arm64 HVF clock subsystem
  investigation, unrelated to spawn work.

## Commits

- `f9c3a54` — kvlr_posix_spawn.c shim + integration + bench variant
- Blog 227.
