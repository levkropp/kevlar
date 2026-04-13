# Kevlar Kernel — Development Guide

## Build & Test

```bash
make run                    # Boot single-CPU in QEMU (KVM)
make run SMP=2              # Boot with 2 CPUs
make check                  # Type-check only (fast)
make build                  # Full build (ELF + bzImage)
make ARCH=arm64 build       # ARM64 build (debug too slow, use RELEASE=1)
make test-threads-smp       # 14-test SMP threading regression suite
make test-xfce PROFILE=balanced  # XFCE desktop test (300s timeout, -smp 2)
```

- Nightly Rust required: `rustup override set nightly`
- x86_64 QEMU CPU: `Icelake-Server`
- KVM default: all test/bench targets use `--kvm`

## Dynamic Analysis Tooling (Milestone T)

Kevlar has built-in diagnostic infrastructure that is **always compiled and runtime-enabled** at boot. When investigating bugs, these tools provide immediate visibility:

### NMI Watchdog (Hard Lockup Detector)
- **What:** Detects CPUs stuck with IF=0 (interrupts permanently disabled)
- **How:** Per-CPU LAPIC heartbeat counters checked every ~4 seconds. Stuck CPU gets an NMI IPI (non-maskable, fires even with IF=0). NMI handler dumps RIP, RSP, RFLAGS, preempt_count, LAPIC registers, backtrace, held locks, and IF transition history.
- **Output:** Look for `NMI WATCHDOG: CPU N STUCK` in serial output
- **Files:** `platform/x64/apic.rs` (heartbeat, watchdog_check, send_nmi_ipi), `platform/x64/interrupt.rs:282` (NMI handler)

### Lock Dependency Validator (Lockdep)
- **What:** Catches lock ordering violations at acquire time (prevents deadlocks)
- **How:** Each lock has a rank (higher = acquired later). Per-CPU held-lock stack verifies no held lock has rank >= the new lock. Panics immediately on violation with the full lock chain.
- **Usage:** `SpinLock::new_ranked(value, rank, "NAME")` — see `platform/lockdep.rs::rank` for constants
- **Key ranks:** TIMERS=10, WAIT_QUEUE=20, SCHEDULER=30, PROCESSES=40, VM=50, PAGE_ALLOC=70
- **Output:** `LOCKDEP: lock ordering violation on CPU N!`
- **Files:** `platform/lockdep.rs`, `platform/spinlock.rs`

### IF-Trace (Interrupt State Tracker)
- **What:** Records every IF (interrupt flag) transition per CPU
- **How:** 256-entry ring buffer per CPU recording CLI/STI/lock acquire/release/idle events with TSC timestamps. Dumped by NMI watchdog handler.
- **Output:** `if-trace: last 32 events for CPU N` showing the exact sequence that led to IF=0
- **Files:** `platform/x64/if_trace.rs`, instrumented in `platform/spinlock.rs` and `platform/x64/idle.rs`

### Stack Guard (Overflow Detection)
- **What:** Detects kernel stack overflows via poison patterns
- **How:** Bottom 512 bytes of every kernel stack filled with `0xDEAD_CAFE_DEAD_CAFE`. Checked every ~1 second from idle loop.
- **Output:** `STACK GUARD: kernel stack overflow detected`
- **Files:** `platform/stack_cache.rs`

### LAPIC Timer Diagnostic
- **What:** Periodic dump of LAPIC timer hardware registers + heartbeat counters
- **How:** `interval_work()` prints LVT, INIT_COUNT, CURR_COUNT, DIV, and per-CPU heartbeat values
- **Flag:** Set `kernel/timer.rs:DIAG_SKIP_SWITCH = true` to test timer without context switches
- **Output:** `LAPIC-DIAG cpu=N LVT=... INIT=... CURR=... HB=[...]`

### Existing Infrastructure
- **Flight recorder:** Per-CPU ring buffer, 12 event kinds — `platform/flight_recorder.rs`
- **ktrace:** Binary tracing (8192 entries/CPU, debugcon transport) — `kernel/debug/ktrace.rs`
- **htrace:** Hierarchical call tracer — `kernel/debug/htrace.rs`
- **Debug events:** JSONL-formatted, filtered via `debug=` cmdline — `kernel/debug/emit.rs`
- **Crash dump:** Base64-encoded between `===KEVLAR_CRASH_DUMP_BEGIN===` sentinels — `kernel/lang_items.rs`

## Architecture

- Rust MIT/Apache-2.0/BSD-2-Clause kernel, forked from Kerla
- Three-ring trust: Platform (Ring 0, unsafe) / Core (Ring 1, safe) / Services (Ring 2, panic-contained)
- `#![deny(unsafe_code)]` on kernel crate; only 7 `unsafe` sites in 4 files
- No SSE in kernel code (`+soft-float`), custom memcpy/memset/memcmp in `platform/mem.rs`
- Demand paging: pages loaded on fault

## Key Lock Ordering (enforced by lockdep)

```
TIMERS (10) → REAL_TIMERS (11) → WAIT_QUEUE (20) → SCHEDULER (30)
  → PROCESSES (40) → EXITED_PROCESSES (41) → VM (50)
  → PROCESS_RESOURCE (60) → PAGE_ALLOC (70) → FILESYSTEM (80) → NETWORK (90)
```

Always acquire locks in ascending rank order. Use `SpinLock::new_ranked()` for new locks.
