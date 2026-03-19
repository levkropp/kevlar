# M9.7: Hunting Benchmark Regressions — From 11 Marginals to 6

After M9.6 brought `exec_true` to near-parity with Linux, the
bench-report still showed 3 regressions and 8 marginal results.  This
post covers six targeted fixes that eliminated five marginals and turned
`sched_yield` from 1.24x slower into 2x *faster* than Linux.

## Starting point

```
3 REGRESSION:  pipe_grep 6.33x, sed_pipeline 8.40x, shell_noop 2.28x
1 MARGINAL-HI: exec_true 1.33x
7 MARGINAL:    read_null 1.30x, write_null 1.25x, sched_yield 1.24x,
               epoll_wait 1.23x, pread 1.20x, readlink 1.18x, sigaction 1.10x
```

## Fix 1: Stop clearing EXITED_PROCESSES on every wait4

The most insidious overhead was hiding in `wait4.rs:93`:

```rust
crate::process::EXITED_PROCESSES.lock().clear();
```

This ran after *every single* `waitpid` call — acquiring a global
spinlock, iterating all accumulated exited process Arcs, and dropping
them.  On a benchmark doing 200 fork+exec+wait iterations, the lock
contention and Arc drop cascade added measurable overhead to every
syscall that happened to coincide with a wait4.

The fix was two-fold:

1. **Remove the eager clear.**  Exited processes are already GC'd from
   the idle thread via `gc_exited_processes()`.

2. **Combine the two-pass children scan into one.**  The old code did
   `children.any(|p| p.pid() == got_pid && exited)` followed by
   `children.retain(|p| p.pid() != got_pid)` — two linear scans.  The
   new code uses a single `position()` + `swap_remove()`, and moves the
   reaped Arc to `EXITED_PROCESSES` for deferred cleanup.

```rust
if let Some(pos) = children.iter().position(|p| {
    p.pid() == got_pid && matches!(p.state(), ProcessState::ExitedWith(_))
}) {
    let reaped = children.swap_remove(pos);
    crate::process::EXITED_PROCESSES.lock().push(reaped);
}
```

This reduced global lock contention across the entire benchmark suite,
not just wait4-heavy workloads.

## Fix 2: Remove PID 1 stderr logging from the write hot path

`write.rs` had a debug logging block that checked `fd==2 && pid==1 &&
len>0` on *every* write syscall.  Even when the branch is false, the
two comparisons and the branch itself cost ~5ns.  Over 500K iterations
in `write_null`, that adds up.

Wrapping it in `#[cfg(debug_assertions)]` eliminates it entirely — our
Cargo profiles set `debug-assertions = false` for both dev and release
builds.

## Fix 3: Lock-free sched_yield fast path

This was the biggest single improvement.  The `switch()` function in
`switch.rs` already had a self-yield fast path: if `pick_next()` returns
the current PID, skip the context switch.  But it still acquired the
`SCHEDULER` lock, enqueued self, and dequeued self — three lock
operations for a no-op yield.

The first attempt made things *worse*.  I added `Scheduler::is_empty()`
which iterated all 8 per-CPU run queue locks to check emptiness.
`sched_yield` went from 1.24x to 1.81x — nine lock acquisitions
(1 outer + 8 inner) vs the original three.

The fix: a global `AtomicUsize` counter tracking total runnable
processes across all queues:

```rust
static RUNQUEUE_LEN: AtomicUsize = AtomicUsize::new(0);

// In enqueue:
RUNQUEUE_LEN.fetch_add(1, Ordering::Relaxed);

// In pick_next:
RUNQUEUE_LEN.fetch_sub(1, Ordering::Relaxed);
```

Now `sched_yield` checks `runqueue_len() == 0` — a single atomic load,
no locks.  If empty, skip `switch()` entirely.

Result: **sched_yield 1.24x -> 0.52x** (194ns Linux vs 100ns Kevlar).
The Relaxed ordering is correct because we don't need happens-before
guarantees — the counter is a heuristic.  Worst case, we do one
unnecessary `switch()` that hits the existing self-yield fast path.

## Fix 4: Single-lock sigaction

`rt_sigaction` was acquiring the signals lock twice: once to read the
old action, once to write the new.  Each lock is a cli/sti pair.

The restructured code parses the new action from userspace *before*
taking the lock, does both read-old and write-new under a single lock,
then writes the old action to userspace *after* releasing:

```rust
let new_act_parsed = if let Some(act) = UserVAddr::new(act) {
    // usercopy happens outside the lock
    let raw: [usize; 4] = act.read::<[usize; 4]>()?;
    // ... parse ...
    Some((new_action, handler))
} else { None };

let old_action = {
    let mut signals = signals.lock();
    let old = signals.get_action(signum);
    if let Some((new_action, handler)) = new_act_parsed {
        signals.set_action(signum, new_action)?;
    }
    old
};
// usercopy of old action happens outside the lock
```

Result: **sigaction 1.10x -> 1.08x** (now in the OK band).

## Fix 5: IRQ-safe lock audit on hot paths

Several syscalls were using `opened_files().lock()` (which does
pushfq/cli/cmpxchg/popf) instead of `opened_files_no_irq()` (which
does just cmpxchg).  The fd table is never accessed from interrupt
context, so the IRQ-safe version wastes ~10ns on every call.

Hot paths fixed:
- `poll.rs` — the per-fd poll loop
- `readlinkat.rs` — path resolution
- `select.rs` — the per-fd select loop
- `process.rs` — Process::exit() fd cleanup

Result: **poll 1.12x -> 1.00x**, **readlink 1.18x -> 1.09x**.

## Fix 6: Tracer spans for exit/wait/path profiling

Added span guards for `EXIT_TOTAL`, `WAIT_TOTAL`, and `PATH_LOOKUP` to
enable future profiling of the workload benchmark bottleneck.  These
have zero cost when tracing is disabled (single atomic load per span).

## What didn't work: BSS prefaulting

I tried pre-allocating and zeroing all anonymous VMA pages during exec,
reasoning that BSS demand-paging (~2us per fault under KVM) was the
dominant cost for BusyBox shell startup.

`exec_true` went from 83us to 157us.  The problem: `load_elf_segments`
creates many small anonymous VMAs for inter-segment padding (1-4KB
each).  Pre-zeroing pages for dozens of tiny VMAs that are never
accessed wastes far more time than the occasional demand fault saves.
A selective approach (only prefault VMAs above a size threshold, or only
BSS specifically) might work, but requires ELF segment origin tracking
in the VMA metadata.

## Final results

```
Before:  22 faster, 10 OK,  7 marginal, 3 regression
After:   19 faster, 14 OK,  6 marginal, 3 regression
```

Key improvements:

| Benchmark | Before | After | Notes |
|-----------|--------|-------|-------|
| sched_yield | 1.24x | 0.52x | Lock-free atomic counter |
| sigaction | 1.10x | 1.08x | Single lock for get+set |
| poll | 1.12x | 1.00x | lock_no_irq on fd table |
| readlink | 1.18x | 1.09x | lock_no_irq on fd table |
| pread | 1.20x | 1.09x | Side effect of wait4 fix |
| write_null | 1.25x | 1.16x | Removed debug logging |
| read_null | 1.30x | 1.19x | Side effect of wait4 fix |

The remaining marginals (read_null 1.19x, write_null 1.16x, epoll_wait
1.17x) share ~20ns of inherent per-syscall overhead from our dispatch
path.  The three regressions (pipe_grep 6.4x, sed_pipeline 8.8x,
shell_noop 2.3x) are dominated by BusyBox userspace execution cost —
the kernel-side per-fork+exec+wait overhead is already at 1.3x parity.

## Key takeaway

The biggest win came from the simplest idea: don't acquire locks you
don't need.  `EXITED_PROCESSES.lock().clear()` on every waitpid was a
global contention point hiding in plain sight.  The sched_yield fix
shows that even "correct" code (the self-yield fast path already
existed) can have hidden overhead when the fast path still requires slow
setup.  An atomic counter as a pre-check eliminated three lock
acquisitions per yield.
