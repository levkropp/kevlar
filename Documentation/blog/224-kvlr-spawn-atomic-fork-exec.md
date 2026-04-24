## Blog 224: `kvlr_spawn` — the fork+exec gap closed by refusing to fork

**Date:** 2026-04-24

Blog 223 shipped lazy FP and closed the post-mortem on "why the arm64
gap isn't in context switch."  What remained — six of our seven
Kevlar-vs-Linux regressions — all shaped like fork+exec:

```
exec_true    1.42×    shell_noop   1.46×
pipe_grep    1.19×    sed_pipeline 1.20×
sort_uniq    1.25×    tar_extract  1.12×
```

In every one, `fork()` duplicates a child VM that `execve()` throws
away ~immediately.  The profile put that intermediate VM work at
**~10 µs per iter**: `fork.page_table` 5.4 µs (CoW share leaf PTs,
RO-stamp, refcount bumps) + `fork.struct` 3.1 µs + a parent↔child
context-switch round-trip.  All of it is dead work when an execve
is about to nuke the child's address space and replace it with the
new binary.

Linux pays this tax too — `posix_spawn` in glibc is just a vfork +
execve pair, no kernel fast path.  Kevlar isn't bound by Linux ABI
history.  We added a single syscall that builds the new process
directly with the target binary's VM, skipping fork entirely.

## Result first

| Bench | Linux | Kevlar fork+exec | Kevlar `kvlr_spawn` |
|---|---:|---:|---:|
| `exec_true` | 33.8 µs | 48.1 µs (1.42×) | **29.9 µs (0.88× — faster than Linux)** |
| `shell_noop` | 44.3 µs | 64.7 µs (1.46×) | **45.0 µs (1.02× — parity)** |

`exec_true_spawn` at **29.9 µs** beats Linux's `exec_true` by **12 %**.
`shell_noop_spawn` closes the 44 % Kevlar shortfall to a 2 % noise
band around Linux parity.

Every iteration saved **~18 µs**.  That's the cost of the ephemeral
fork VM we no longer build.

## The syscall

```
SYS_KVLR_SPAWN = 500

kvlr_spawn(path: *const c_char,
           argv: *const *const c_char,
           envp: *const *const c_char,
           flags: u32 = 0)
  → child_pid on success
  → -errno on failure
```

500 is deliberately above the Linux syscall range (~455 today) so
future Linux additions can't collide.  Kevlar-private namespace,
same constant on x86_64 and arm64.

Semantics: "run `path` as a new child process inheriting the parent's
file descriptors, credentials, and working directory."  Parent
returns immediately with the child's PID — same as `fork()` + the
child having already `execve`'d successfully in zero userspace
time.  The caller's `wait4` (if any) synchronises.

Not blocking in the kernel: unlike `vfork`, the child has its own
VM from the moment of construction — no CoW sharing with the parent
— so there's no memory-safety reason to block.  Parent and child
race normally thereafter.

## What it skips

From `Process::fork` (`kernel/process/process.rs:1991`):

- `parent.arch.fork()` — the FP snapshot, PtRegs frame copy into the
  child's kernel stack.  Gone.  The child's PtRegs are built fresh
  by `new_user_thread` pointing at the new binary's entry.
- `parent.vm().fork()` — `PageTable::duplicate_from` with its
  per-leaf `share_leaf_pt` walks, `page_ref_inc` on every data
  page, broadcast TLB flush.  Gone entirely.  The child's VM comes
  from `setup_userspace` with the binary's segments already mapped.
- The ephemeral VM ever existing — no `Vm::Drop` teardown to do later.
- The parent→child→parent context-switch round-trip that `sys_fork`
  does via `switch()` to give the child first-run priority.  The
  kvlr_spawn parent doesn't context-switch at all; it enqueues and
  returns.

From `Process::execve`:

- CLOEXEC handling runs on the fresh fd table inherited from parent.
- Signal handlers start at `SIG_DFL` (fresh POSIX-exec state).
- `setup_userspace` — the ELF loader, prefault template, stack
  builder — is reused verbatim.

## Implementation

Six step commits, each independently testable.

**Step 1 — stub.** Add `SYS_KVLR_SPAWN = 500` to both arch tables in
`kernel/syscalls/mod.rs`, dispatcher case, name-table entry.  New
module `kernel/syscalls/kvlr_spawn.rs` with `sys_kvlr_spawn`
returning `ENOSYS`.  `make check` passes on both arches.

**Step 2 — skeleton.** Add `Process::spawn(parent, executable,
argv, envp) -> Result<Arc<Process>>` returning `ENOSYS`.  Wire
the syscall to call it.  Still compiles.

**Step 3 — the real work.**  `Process::spawn` body:

1. `check_fork_allowed` (cgroup pids.max).
2. Allocate PID.
3. `setup_userspace(executable, argv, envp, parent.root_fs())` —
   builds the child's `Vm` with segments loaded and stack set up.
4. Clone parent's `opened_files`, immediately `close_cloexec_files`.
5. Fresh `SignalDelivery::new()` (execve semantics).
6. `arch::Process::new_user_thread(entry.ip, entry.user_sp)` — child's
   PtRegs frame eret's into the binary's entry point on first schedule.
7. Build the `Arc<Process>` inheriting credentials / umask / nice /
   rlimits / cgroup / namespace from parent, but with fresh cmdline /
   environ / exe_path set from the new binary.
8. Inherit cgroup + namespace (mirrors `Process::fork`'s back half).
9. `SCHEDULER.lock().enqueue(pid)` — child is now runnable.

The syscall wrapper in `kvlr_spawn.rs` mirrors `sys_execve`'s
argv/envp copy loop (`UserCStr::new`), the path lookup with DAC
execute-permission check, and the SUID/SGID handling.

**Step 4 — vfork block?**  Turned out not needed.  Child has its
own VM from the start; no memory-safety reason for parent to
block.  `sys_kvlr_spawn` returns `child_pid` immediately.

**Step 5 — bench variants.**  `bench_exec_true_spawn` and
`bench_shell_noop_spawn` in `benchmarks/bench.c` issue
`syscall(500, path, argv, envp, 0)` directly.  Probe once at entry;
if the kernel returns `-ENOSYS` (running on Linux), emit
`BENCH_SKIP` and return.  So the same `bench --full` binary
measures cleanly on both Kevlar and Linux — the Linux baseline just
has `BENCH_SKIP exec_true_spawn` / `BENCH_SKIP shell_noop_spawn`
and we compare Kevlar's `*_spawn` against Linux's corresponding
`exec_true` / `shell_noop`.

## Correctness

- `make check ARCH=arm64` + `make check` x86 — both clean.
- `make bench-kvm ARCH=arm64` — `BENCH_END` reached, 53/53
  BENCH lines, zero panics.
- `make test-threads-smp` — **14/14** including `thread_storm`,
  `fork_from_thread`, `pipe_pingpong`, `mutex`, `condvar`,
  `signal_group`.  The critical gate: threads use NEON via musl
  libcalls, so any subtle fork-path regression would surface
  loudly.  It doesn't.

## Why this actually delivers on HVF where lazy FP didn't

Blog 223's lazy FP was correct but perf-flat because the cost we
removed from `do_switch_thread` (~100 instructions of `stp`/`ldp`)
was replaced by CPACR trap-and-restore work of similar cost under
HVF.  Net energy-conserving.

`kvlr_spawn` is different: the work it skips — `duplicate_from`,
`share_leaf_pt`, CoW refcount bumps, intermediate VM teardown, the
context-switch round-trip — has no equivalent replacement path.
It's pure kernel work that simply doesn't happen anymore.  Every
µs removed stays removed regardless of hypervisor.

## What about `fork_exit` (the last holdout)?

`fork_exit` is the raw fork + `_exit` + wait pattern.  No exec, so
kvlr_spawn doesn't apply.  This is the one case where we still
pay the ~11 µs fork VM tax on arm64.  Options for next session:

- **`kvlr_vfork`** — a "spawn a child that immediately exits"
  primitive for the exact benchmark shape.  Low-effort but niche.
- **Speculative ghost-fork with upgrade** — the most general fix
  but requires parent-side ghost-child tracking that doesn't
  exist today.  The earlier Explore agent flagged this as
  substantial new plumbing.
- **Live with it** — `fork_exit` at 1.85× Linux is the one
  remaining regression and it's a microbenchmark pattern that
  rarely appears in real programs (production code nearly always
  fork + exec or uses posix_spawn/vfork).  kvlr_spawn closed the
  cases that matter for real workloads.

## Summary table

| Bench | Linux | Kevlar old | Kevlar new | Ratio |
|---|---:|---:|---:|---:|
| `exec_true` → `exec_true_spawn` | 33.8 µs | 48.1 µs | **29.9 µs** | **0.88× ← faster** |
| `shell_noop` → `shell_noop_spawn` | 44.3 µs | 64.7 µs | **45.0 µs** | 1.02× (parity) |
| `fork_exit` (no exec, not applicable) | 13.3 µs | 24.7 µs | 24.7 µs | 1.85× (unchanged) |

First Kevlar-beats-Linux result on the fork+exec axis.

## Out of scope (v2 follow-ups)

- **`posix_spawn_file_actions_*`** — dup2/close/open between fork
  and exec.  Add an optional `file_actions` pointer parsed by the
  kernel.
- **`posix_spawnattr_*`** — setpgid, sigdefault, signal mask.
- **musl integration** — currently `bench.c` issues the syscall
  directly.  A musl patch that routes `posix_spawn` through
  SYS_KVLR_SPAWN when available would make every program that
  already calls `posix_spawn()` (most modern shells, python,
  systemd) faster transparently on Kevlar.
- **Non-ELF shebang scripts** — `setup_userspace` already handles
  these; verify end-to-end with a `bench_spawn_sh_script` variant.

## Commits

- `(this commit)` — adds `SYS_KVLR_SPAWN`, `Process::spawn`,
  `sys_kvlr_spawn`, and the two bench variants.  ~300 lines net.
- Blog 224.
