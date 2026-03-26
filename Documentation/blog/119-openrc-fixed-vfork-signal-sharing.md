# Blog 119: OpenRC fixed — CLONE_VFORK shared signal handlers with parent

**Date:** 2026-03-25
**Milestone:** M10 Alpine Linux

## Summary

The OpenRC INVALID_OPCODE crash that has persisted since Alpine integration
is **fixed**. Root cause: `CLONE_VFORK` shared the signal handler table with
the parent process via `Arc::clone`. When busybox (exec'd by the vfork child)
registered its own SIGCHLD handler, it overwrote the parent's signal
disposition. The parent (openrc) then jumped to busybox's handler address —
unmapped in openrc's address space — causing #UD.

One-line fix: only share signals for `CLONE_THREAD`; create an independent
copy for `CLONE_VFORK`. All tests pass, OpenRC boots cleanly through all
three runlevels (sysinit, boot, default).

## The Bug

### Linux's clone flags and signal sharing

On Linux, signal handler sharing is controlled by `CLONE_SIGHAND`:

| Flag | Signal table | Use case |
|------|-------------|----------|
| `CLONE_THREAD \| CLONE_SIGHAND` | Shared | pthreads |
| `CLONE_VFORK \| CLONE_VM` | **Independent** | posix_spawn |
| `fork()` (no flags) | Independent | fork |

Kevlar's `new_thread()` function handled both `CLONE_THREAD` and
`CLONE_VFORK` with the same code — always sharing the signal table:

```rust
signals: Arc::clone(&parent.signals),  // BUG: shared for ALL new_thread calls
```

### The crash sequence

1. **OpenRC** (PID 7, PIE binary at `0xa00000000`) calls `system("rc-depend ...")` to scan service dependencies
2. musl's `system()` → `posix_spawn()` → `CLONE_VFORK`
3. The **vfork child** shares OpenRC's signal table (via `Arc::clone`)
4. The child `exec`'s `/bin/sh` (Alpine's busybox, PIE span `0xc7000`)
5. busybox's startup calls `sigaction(SIGCHLD, {handler=0xa000411f1})` — a valid busybox function
6. Because the signal table is SHARED, this overwrites **OpenRC's** SIGCHLD disposition
7. OpenRC's child exits → SIGCHLD delivered to OpenRC
8. The kernel jumps to `0xa000411f1` — a valid address in busybox but **unmapped in OpenRC** → `INVALID_OPCODE`

### Why the handler address was bogus

The handler `0xa000411f1 = 0xa00000000 + 0x411f1` is offset `0x411f1` in the
loaded PIE binary. For busybox (span `0xc7000`), this is within the code
section — a valid signal handler function. For openrc (span `0xb000`), this
offset is far beyond the binary's code — in unmapped memory that later gets
mapped to ld-musl's timezone code at a mid-instruction boundary.

## Investigation Trail

This bug took **5 sessions** to fully diagnose. The investigation path:

| Session | Hypothesis | Finding |
|---------|-----------|---------|
| 1 | Stack overflow | ✗ Stack was fine; 16KB kernel_stack change didn't help |
| 2 | Signal delivery corruption | ✗ No signals delivered to PID 7 before crash |
| 3 | Demand paging / PAGE_CACHE | ✗ Page content matched file; no cache involvement |
| 4 | Dynamic linker relocation | ✗ musl's `lea __restore_rt` computed correctly |
| **5** | **CLONE_VFORK signal sharing** | **✓ The fix** |

### Key GDB findings that led to the fix

1. **Watchpoint on `frame.rip`**: Caught `setup_signal_stack(signal=17)` writing
   the bogus handler to PID 7's syscall return frame
2. **Syscall entry/exit comparison**: `frame.rcx` (correct return addr from hardware)
   ≠ `frame.rip` (corrupted by signal delivery) — proved corruption, not stack overflow
3. **`rt_sigaction` kernel tracing**: Every busybox process registered `handler=0xa000411f1`;
   openrc processes registered `handler=0` (SIG_DFL) or `handler=0xa00006ca8` (correct)
4. **`SIG_DELIVER` tracing**: SIGCHLD was delivered to PID 7 (`openrc sysinit`) with
   busybox's handler address — even though PID 7 never called `sigaction(SIGCHLD)`
5. **`EXEC_PIE` tracing**: busybox span = `0xc7000`, openrc span = `0xb000` — confirmed
   the handler was from the wrong binary

### Tools used

- `tools/gdb-run.py` — autonomous GDB investigation runner (5 different plans)
- Kernel-level tracing: `rt_sigaction`, `SIG_DELIVER`, `EXEC_PIE`, `PF_TRACE`, `PF_ANON`
- Hardware watchpoints on kernel stack (frame.rip write detection)
- Hardware breakpoints at `sysretq`, `pop rcx`, `handle_user_fault`

## The Fix

```rust
// kernel/process/process.rs — new_thread()
signals: if is_thread {
    // CLONE_THREAD (pthreads): share signal handlers — per POSIX,
    // all threads in a group share signal dispositions.
    Arc::clone(&parent.signals)
} else {
    // CLONE_VFORK or other non-thread clone: independent copy.
    // On Linux, only CLONE_SIGHAND shares signal handlers;
    // vfork uses CLONE_VM but not CLONE_SIGHAND.
    Arc::new(SpinLock::new(parent.signals.lock_no_irq().fork_clone()))
},
```

## Other fixes in this session

### Correct signal types for user faults (kept from session 3)

`handle_user_fault` now maps x86 exception vectors to POSIX signals:
INVALID_OPCODE → SIGILL, DIVIDE_ERROR → SIGFPE (was all SIGSEGV).

## Test Results

| Suite | Result |
|-------|--------|
| Contract tests | **159/159 PASS** |
| Alpine APK + OpenRC boot | **ALL PASS** (29/29 ext4, curl HTTP, 3 runlevels) |
| OpenSSL/TLS | **18/18 PASS** |
| M10 APK (ext2) | **7/7 PASS** |

### OpenRC boot output (no crashes!)

```
* /run/openrc: creating directory
* Caching service dependencies ...    ← sysinit (was crashing here)
* Caching service dependencies ...    ← boot
* Caching service dependencies ...    ← default
```
