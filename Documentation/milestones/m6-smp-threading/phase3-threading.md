# Phase 3: Threading Primitives

**Goal:** Implement clone(CLONE_VM | CLONE_THREAD) so userspace programs can
create threads sharing the same address space. Support the musl/glibc
threading ABI (TLS, set_tid_address, robust futex).

## clone Flags to Implement

| Flag | Value | Meaning |
|------|-------|---------|
| `CLONE_VM` | 0x00000100 | Share address space (memory) |
| `CLONE_FS` | 0x00000200 | Share filesystem info (cwd, umask) |
| `CLONE_FILES` | 0x00000400 | Share file descriptor table |
| `CLONE_SIGHAND` | 0x00000800 | Share signal handlers |
| `CLONE_THREAD` | 0x00010000 | Same thread group (TGID) |
| `CLONE_SETTLS` | 0x00080000 | Set TLS (arch_prctl on x86_64) |
| `CLONE_PARENT_SETTID` | 0x00100000 | Write TID to parent's memory |
| `CLONE_CHILD_CLEARTID` | 0x00200000 | Clear TID and futex-wake on exit |
| `CLONE_CHILD_SETTID` | 0x01000000 | Write TID to child's memory |

## Process Model Changes

Currently, each `Process` has its own:
- Address space (page table)
- File descriptor table
- Signal handlers

For threads, these must be **shareable** via `Arc`:

```rust
pub struct Process {
    pid: PId,
    tgid: PId,  // NEW: thread group ID (= PID of first thread)

    // Shared between threads when CLONE_VM is set.
    vm: Arc<SpinLock<VirtualMemory>>,

    // Shared between threads when CLONE_FILES is set.
    opened_files: Arc<SpinLock<OpenedFileTable>>,

    // Shared between threads when CLONE_SIGHAND is set.
    // Per-thread: signal mask, pending signals.
    signal_handlers: Arc<SpinLock<SignalHandlers>>,
    signal_mask: AtomicU64,       // per-thread
    signal_pending: AtomicU32,    // per-thread

    // Thread exit notification.
    clear_child_tid: AtomicU64,   // address to clear + futex-wake on exit

    // ... existing fields
}
```

### Thread Group

Threads with CLONE_THREAD share a TGID (the PID of the thread group leader).
`getpid()` returns TGID; `gettid()` returns the per-thread PID.

Maintain a thread group list:

```rust
pub struct ThreadGroup {
    leader_pid: PId,
    threads: SpinLock<Vec<Weak<Process>>>,
}
```

`exit_group()` kills all threads in the group. `kill(pid)` delivers the
signal to any thread in the group (preferring the one with it unblocked).

### clone(CLONE_VM) Implementation

```rust
fn sys_clone(flags, child_stack, parent_tidptr, child_tidptr, tls) {
    let parent = current_process();

    let child = if flags.contains(CLONE_VM) {
        // Thread: share address space, fd table, signal handlers.
        Process {
            pid: alloc_pid(),
            tgid: parent.tgid,
            vm: parent.vm.clone(),           // Arc clone (shared)
            opened_files: if flags.contains(CLONE_FILES) {
                parent.opened_files.clone()  // Arc clone (shared)
            } else {
                Arc::new(SpinLock::new(parent.opened_files.lock().clone()))
            },
            // Per-thread: own signal mask, pending signals
            signal_mask: AtomicU64::new(parent.signal_mask.load()),
            signal_pending: AtomicU32::new(0),
            // ...
        }
    } else {
        // Process: copy-on-write (existing fork behavior)
        parent.fork()?
    };

    // CLONE_SETTLS: set thread-local storage pointer.
    if flags.contains(CLONE_SETTLS) {
        // x86_64: write FS base for the new thread
        child.set_fs_base(tls);
    }

    // CLONE_PARENT_SETTID: write child's TID to parent's address space.
    if flags.contains(CLONE_PARENT_SETTID) {
        parent_tidptr.write::<i32>(&child.pid.as_i32())?;
    }

    // CLONE_CHILD_SETTID: write child's TID to child's address space.
    if flags.contains(CLONE_CHILD_SETTID) {
        child_tidptr.write::<i32>(&child.pid.as_i32())?;
    }

    // CLONE_CHILD_CLEARTID: store address for exit notification.
    if flags.contains(CLONE_CHILD_CLEARTID) {
        child.clear_child_tid.store(child_tidptr.value() as u64, Ordering::Relaxed);
    }

    // Set child's stack pointer to child_stack (threads get their own stack).
    if child_stack != 0 {
        child.set_user_stack(child_stack);
    }

    // Enqueue child for scheduling.
    enqueue(child);
    Ok(child.pid.as_i32() as isize)
}
```

### Thread Exit (CLONE_CHILD_CLEARTID)

When a thread exits:

```rust
fn thread_exit() {
    let tid_addr = current_process().clear_child_tid.load(Ordering::Relaxed);
    if tid_addr != 0 {
        // Write 0 to the tid address (signals pthread_join that we exited).
        let addr = UserVAddr::new(tid_addr as usize);
        let _ = addr.write::<i32>(&0);
        // Futex wake on that address (wakes pthread_join waiters).
        futex_wake(addr, 1);
    }
}
```

This is how pthreads implements `pthread_join` — it sleeps in `futex_wait`
on the TID word, and the kernel wakes it when the thread exits.

## TLS (Thread-Local Storage)

### x86_64

TLS is accessed via the FS segment register. Each thread needs its own FS
base pointing to its thread control block (TCB):

```rust
// In arch_prctl:
fn set_fs_base(addr: u64) {
    // Write to MSR_FS_BASE
    wrmsr(0xC0000100, addr);
    // Also save in Process struct for context switch restore
    current_process().fs_base.store(addr, Ordering::Relaxed);
}
```

Context switch must save/restore FS base (we likely already do this for
fork, but verify).

### ARM64

TLS is via TPIDR_EL0 (user-accessible thread pointer):

```rust
fn set_tls(addr: u64) {
    write_sysreg!(tpidr_el0, addr);
    current_process().tls.store(addr, Ordering::Relaxed);
}
```

## Futex Improvements

The current futex implementation supports FUTEX_WAIT and FUTEX_WAKE. For
proper threading, also need:

- **FUTEX_WAIT with timeout:** Already needed for `pthread_cond_timedwait`.
- **FUTEX_WAKE_OP:** Atomic compare-and-wake, used internally by glibc.
  Can stub initially (return ENOSYS, musl doesn't require it).
- **Robust futex list:** `set_robust_list` (already stubbed). On thread exit,
  walk the robust list and wake any futexes the thread held. Important for
  crash safety but can remain a stub initially.
- **Private futex (FUTEX_PRIVATE_FLAG):** Optimization hint that the futex
  is process-local. Accept the flag but treat all futexes the same.

## Syscalls

| Syscall | x86_64 | arm64 | Priority |
|---------|--------|-------|----------|
| `clone` (enhanced) | 56 | 220 | Required |
| `clone3` | 435 | 435 | Nice-to-have |
| `gettid` (real) | 186 | 178 | Required |
| `tgkill` | 234 | 131 | Required |
| `exit_group` (real) | 231 | 94 | Required |
| `set_tid_address` (real) | 218 | 96 | Required |
| `arch_prctl` (x86_64) | 158 | N/A | Required |
| `futex` (enhanced) | 202 | 98 | Required |

## Reference Sources

- Linux man pages: clone(2), futex(2) — flag specifications and semantics
- musl `src/thread/` — pthread implementation (MIT license)
- OSDev wiki — Threading, TLS

## Testing

- Create thread with clone(CLONE_VM|CLONE_THREAD|...) — new thread runs,
  modifies shared memory, parent sees the change
- `gettid()` returns different values for parent and child thread
- `getpid()` returns same value (TGID) for both
- Thread exit with CLONE_CHILD_CLEARTID wakes futex waiter
- Static musl program using pthreads: create N threads, each increments a
  shared atomic counter, join all, verify count
