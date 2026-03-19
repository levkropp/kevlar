# 084: Ghost-Fork Signal Masking and the libc Barrier

## Context

Ghost-fork is an optimization that skips page table duplication on `fork()` by
sharing the parent's VM with the child (vfork semantics). The parent blocks
until the child calls `exec()` or `_exit()`. For fork+exec workloads (which is
nearly all forks), this eliminates ~14µs of wasted page table copying.

The infrastructure was fully implemented but disabled (`GHOST_FORK_ENABLED =
false`) because a signal-related busy-spin made it unusable. This session fixed
the signal bug, revealed a deeper libc incompatibility, and confirmed the vfork
path is now correct.

## Bug 1: Signal-induced EINTR spin (fixed)

The ghost-fork and vfork wait loops both used `sleep_signalable_until`:

```rust
while !child.ghost_fork_done.load(Ordering::Acquire) {
    let _ = VFORK_WAIT_QUEUE.sleep_signalable_until(|| {
        if child.ghost_fork_done.load(Ordering::Acquire) {
            Ok(Some(()))
        } else {
            Ok(None)
        }
    });
}
```

If any signal was pending (e.g. SIGALRM from a timer), `sleep_signalable_until`
returns `Err(EINTR)` immediately at the top of its loop — before ever sleeping.
The outer `while` loop discards the error and retries. Since the signal stays
pending until delivered, the loop spins at 100% CPU forever.

**Fix:** Temporarily block all signals during the wait using the existing
atomic signal mask:

```rust
let saved_mask = current.sigset_load();
current.sigset_store(SigSet::ALL);
// ... wait loop ...
current.sigset_store(saved_mask);
```

This works because:
- `signal_pending` bits are set by `send_signal` regardless of the mask —
  signals are queued, never lost
- `has_pending_signals()` returns `signal_pending & !blocked_mask`; with ALL
  blocked, this is always 0, so `sleep_signalable_until` actually sleeps
- After restoring the mask, `try_delivering_signal` on syscall return delivers
  any queued signals — correct POSIX semantics matching Linux vfork behavior
- SIGKILL delivery delayed by <1ms (child exec time) matches Linux vfork

Added `SigSet::ALL` (`!0u64`) constant for this pattern.

## Bug 2: libc fork wrapper corrupts shared state (fundamental)

With the signal fix in place, enabling ghost-fork immediately crashed the
`fork_exit` benchmark with a GPF in the parent process (PID 1):

```
BENCH pipe 256 91716 358
USER FAULT: GENERAL_PROTECTION_FAULT pid=1 ip=0x40520c
PID 1 (/bin/bench --full) killed by signal 11
```

**Root cause:** musl's `fork()` wrapper modifies thread-local storage and global
libc state in the child after the syscall returns:

```c
// musl __fork() — runs in child after kernel returns 0
if (!ret) {
    self->tid = __syscall(SYS_set_tid_address, &self->tid_addr);
    self->robust_list.off = 0;
    self->robust_list.pending = 0;
    self->next = self->prev = self;
    libc.need_locks = -1;
    // ... more global state modifications
}
```

With ghost-fork, the child shares the parent's entire address space. These
writes go to the same physical memory as the parent's TLS and libc globals.
When the parent resumes after `ghost_fork_done`, its libc state is corrupted:
`self->tid` has the child's value, `libc.need_locks` is -1, the thread list is
broken. Any subsequent libc call hits corrupted state → GPF.

**This is inherent, not fixable at the kernel level.** Any C library with a
fork() wrapper that modifies process state will corrupt the shared address
space. This affects musl, glibc, uclibc — all of them.

## Why vfork is different

`vfork()` works correctly with shared VM because:

1. **Callers follow the vfork contract**: only `_exit()` or `exec()` before
   returning. No libc state modification.
2. **musl's vfork wrapper is minimal**: uses `clone(CLONE_VM | CLONE_VFORK)`
   with no post-syscall state modification in the child.
3. **exec replaces the address space**: the child gets its own VM before any
   libc initialization runs.

The signal masking fix protects this path correctly.

## Outcome

**Ghost-fork remains disabled for `fork()`** — the libc barrier is fundamental.

**Signal masking fix landed for both paths** — `sys_fork` (guarded by the
disabled flag) and `sys_vfork` (always active). The vfork busy-spin bug that
existed since vfork was implemented is now fixed.

**Benchmark results (44/44 pass, 0 regressions):**

| Category | Count | Highlights |
|----------|-------|------------|
| Faster than Linux KVM | 29 | brk 460x, mmap_fault 107x, signal_delivery 2.2x |
| Within 10% of Linux | 15 | All workloads (exec_true, shell_noop, etc.) |
| Marginal or regression | 0 | Clean sweep |

`fork_exit` at 44.7µs (0.91x Linux) — about 10% faster than Linux even
without ghost-fork, thanks to stack caching and lock elision from earlier
sessions.

## Files changed

| File | Change |
|------|--------|
| `kernel/process/signal.rs` | Added `SigSet::ALL` constant |
| `kernel/syscalls/fork.rs` | Signal masking around ghost-fork wait |
| `kernel/syscalls/vfork.rs` | Signal masking around vfork wait |
| `kernel/process/process.rs` | Updated comment documenting libc barrier |

## Lessons

1. **vfork semantics cannot be transparently applied to fork()** — the kernel
   can share page tables, but it can't prevent libc from modifying the shared
   address space in the child. Any optimization that shares VM on fork must
   either (a) intercept the libc wrapper or (b) use CoW on the stack/TLS pages.

2. **Signal masking is the correct pattern for kernel-internal waits** where
   you need sleep_signalable semantics (for the wait queue) but don't want
   signals to cause EINTR. Linux does the same thing in its vfork implementation.

3. **Test the hot path, not just the happy path** — the signal spin only
   manifests when a signal happens to be pending during the wait, which requires
   real workload testing (timers, child SIGCHLD) to trigger.
