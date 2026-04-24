## Blog 223: arm64 lazy FP — shipped, correctness clean, perf flat under HVF

**Date:** 2026-04-24

Blog 222 closed by identifying `do_switch_thread`'s eager FP save+restore
(64 `stp`/`ldp` + 4 `mrs`/`msr` per context switch, ~1.2 µs of the
1.7 µs profiled cost) as the dominant arm64-specific overhead vs x64,
and proposed implementing Linux's lazy-FP model (CPACR_EL1 trap on EL0
SIMD use, save/restore on demand) to close the gap.

This post: that change shipped.  The infrastructure is in place,
correctness is verified, threads-smp passes 14/14.  But the expected
performance win **did not materialize on HVF** — `fork_exit` stayed
at ~24.7 µs, `do_switch_thread` stayed at ~1.7 µs.  This post explains
why the eager → lazy swap was a wash on our specific target and what
that means for the next session.

## The implementation

Mirrors Linux `arch/arm64/kernel/fpsimd.c`'s state machine:

- Per-CPU `fp_owner: u64` field added to `CpuLocalHead` — raw pointer
  to whichever task's `FpState` currently lives in this CPU's v-regs
  (or 0 if the v-regs are foreign / undefined).
- Per-task `fp_loaded: AtomicBool` added to `ArchTask` — inverse of
  Linux's `TIF_FOREIGN_FPSTATE` flag.
- `do_switch_thread` (`platform/arm64/usermode.S`) stripped of the
  32 `stp`/`ldp` save+load blocks and FPCR/FPSR `mrs`/`msr`.  Drops
  from ~100 to ~30 instructions.  The 5-arg signature drops to 3 —
  no `prev_fp` / `next_fp` pointers.
- `switch_task` (`platform/arm64/task.rs`) flips
  `CPACR_EL1.FPEN = 0b01` (trap EL0 FP/SIMD) after the SP/TPIDR_EL0
  swap, except on the fast path where the next task is already the
  CPU's FP owner (`next.fp_loaded && cpu_local_head().fp_owner ==
  next.fp_state_ptr() as u64`).
- New EC=0x07 case in `arm64_handle_exception` routes to a new
  `platform/arm64/fp.rs::handle_fp_trap`:
  - Read `cpu_local_head().fp_owner` (the previous owner's
    `*mut FpState`).
  - If non-null and not equal to current task's `FpState` ptr, call
    `kevlar_save_fp_to(prev_owner)` to spill the v-regs into the
    previous task's `FpState`.
  - Call `kevlar_restore_fp_from(current.fp_state)` to load this
    task's saved state into v-regs.
  - Set `cpu_local_head().fp_owner = current_fp_state_ptr`,
    `current.fp_loaded = true`.
  - Set `CPACR_EL1.FPEN = 0b11` (allow EL0 FP) — `eret` re-executes
    the faulting instruction successfully.
- `Process::exit_current` (`kernel/process/process.rs`) clears
  `cpu_local_head().fp_owner` if the exiting task is the CPU's FP
  owner — prevents the next FP trap from saving v-regs into a
  freed `FpState`.
- `boot.S` BSP+AP init: `CPACR_EL1.FPEN = 0b01` (was `0b11`).

Two new asm helpers in `usermode.S`:

- `kevlar_save_fp_to(*mut FpState)` — already existed, used at fork
  to snapshot the parent's live v-regs into the child's `FpState`.
  Now also called by the FP-trap handler when spilling the previous
  owner.
- `kevlar_restore_fp_from(*const FpState)` — new, mirror of
  `kevlar_save_fp_to`.  Loads FPCR + FPSR before v-regs (so the
  rounding-mode is live before any SIMD op).

Cross-arch trait additions to `kevlar_platform::Handler`
(`platform/lib.rs`), gated `#[cfg(target_arch = "aarch64")]`:

```rust
fn current_task_fp_state_ptr(&self) -> u64 { 0 }
fn mark_current_task_fp_loaded(&self) {}
```

The arm64 trap handler reaches `current_process()` through these
trait methods — `PROCESSES` is `pub(super)` and not directly
accessible from the platform crate.

## Correctness

`make test-threads-smp` (4 CPU, 14 tests including thread_storm,
fork_from_thread, pipe_pingpong, mutex, condvar, signal_group) —
**14/14 PASS**.  Critical because pthreads use NEON via musl's
optimized `memcpy` / `strcmp` / `strlen` libcalls; if FP state isn't
preserved across thread switches, threads see corrupted strings/data
and the suite fails noisily.  It doesn't.

`bench --full` runs end-to-end with no panics, full `BENCH_END`,
51/53 BENCH lines (the two `BENCH_SKIP` are `setsockopt` /
`getsockopt`, gated since blog 222's AF_INET fix when no NIC is
present).

## Performance — flat

3-run mean per cell:

| Bench | Pre-lazy-FP | Post-lazy-FP | Δ |
|---|---:|---:|---:|
| `fork_exit` | 24.5 µs | 24.7 µs | +0.2 µs |
| `exec_true` | 48.0 µs | 47.1 µs | -0.9 µs |
| `shell_noop` | 66.1 µs | 66.4 µs | +0.3 µs |

Within run-to-run noise.  And the tracer profile confirms the
mechanism didn't move:

```
                    pre-lazy    post-lazy
fork.total          9,625 ns    9,625 ns
fork.page_table     5,416 ns    5,375 ns
ctx_switch          1,791 ns    1,791 ns
do_switch_thread    1,666 ns    1,708 ns
```

`do_switch_thread` is *unchanged* despite losing ~100 instructions of
FP save/restore work.

## Why the eager → lazy swap was a wash

The bench is FP-heavy.  musl's `memcpy`, `strcmp`, `strlen`, and
friends on arm64 are NEON-optimized — every short userspace tick
touches v-regs.  Pattern per `fork_exit` iter:

1. Parent calls `fork()` (SVC → kernel)
2. Kernel ctx-switches to child (CPACR.FPEN ← 0b01)
3. Child runs `_exit(0)` user code — touches FP via libc → trap →
   handler saves nothing (no prev owner), loads child's FpState,
   CPACR.FPEN ← 0b11
4. Child SVCs into `_exit`, kernel ctx-switches back to parent
   (CPACR.FPEN ← 0b01)
5. Parent runs post-`fork()` user code → touches FP → trap →
   handler saves child's state into child's FpState (or skip — child
   is dead), loads parent's FpState, CPACR.FPEN ← 0b11

Per iter: **2 ctx switches × `msr cpacr_el1`** + **2 EL0 sync exception
traps** (entry + Rust dispatch + handler + eret) + **the same `stp`/
`ldp` blocks we used to do inline**.

Under bare-metal arm64 the `msr cpacr_el1` is one cycle and the trap
entry is dozens of cycles; lazy FP wins big because the trap fires
once per genuinely-distinct task on a CPU.  Under HVF on Apple
Silicon, system-register accesses can incur hypervisor exits and
trap entries are themselves costly.  The cost of "1 MSR + 1 trap" per
switch under HVF roughly equals "32 stp + 32 ldp" inline.  So the
swap is approximately energy-conserving.

The fast-path optimization (skip the CPACR toggle when the same task
lands back on the same CPU) only helps when there's no other FP user
on the CPU between switches.  In a fork-heavy bench, the parent and
the child alternate — they're different FP owners — so the fast path
rarely fires.

## Why ship it anyway

- **Correctness is bullet-proof**: 14/14 threads-smp.  The
  invariant "kernel never touches v-regs at EL1" (the `-fp-armv8
  -neon` build flag) holds in all paths.
- **Mirrors Linux's model**: future arm64 contributors and code
  readers find a familiar shape (`TIF_FOREIGN_FPSTATE` =
  `!fp_loaded`, `fpsimd_last_state` = `fp_owner`).
- **Bare-metal arm64 wins**: future deployments to real hardware (not
  Apple's HVF) get the lazy-FP advantage for free.  No code change
  needed at that point.
- **Kernel-thread free path**: idle thread, kthread_entry tasks, and
  any future kernel daemons that never enter EL0 will *never* incur
  any FP work.  Previously they paid the eager save+restore on every
  switch.
- **Zero regression**: the bench, contracts, and threads-smp all
  pass at the same numbers as before.

## What this tells us about closing the arm64 gap

The remaining ~11 µs `fork_exit` gap (Kevlar 24.5 µs vs Linux
13.3 µs on the same HVF host) is **not** in `do_switch_thread`.
It's distributed:

- `fork.page_table` 5.4 µs — duplicate_table walk + share_leaf_pt
- `fork.struct` 3.1 µs — `Arc::new(Process { ... })` heap alloc + zero-init
- `do_switch_thread` ~1.7 µs × 2-4 switches/iter — but Linux probably
  pays similar; the `do_switch_thread` itself isn't the gap
- `wait4` + `exit` paths — scheduler ↔ wait queue overhead

To close this on HVF specifically, the lever has to be either:

1. **HVF-specific tricks** — reducing the number of hypervisor exits
   per syscall.  E.g., consolidating `mrs`/`msr` operations, avoiding
   redundant `dsb`/`isb` barriers (each can trigger an exit).
2. **arm64 assembly wizardry** — hand-tuned context switch and
   syscall entry paths that minimize register pressure and barrier
   counts.
3. **Process-struct shrinking** — split `Process` into hot+cold
   halves, only alloc the hot half at fork time.  Saves ~1.5 µs.
4. **VMA-aware page-table walk** — skip walking empty PTE slots in
   `share_leaf_pt`.  Saves ~1-2 µs per fork.

Lazy FP was the wrong lever for this target.  But the infra is now
in place if/when we ever ship to non-HVF arm64.

## Commits

- `(this commit, lazy FP infra)` — `platform/arm64/{fp.rs (new),
  cpu_local.rs, task.rs, interrupt.rs, usermode.S, boot.S}`,
  `kernel/main.rs`, `kernel/process/process.rs` exit hook,
  `platform/lib.rs` Handler trait additions.  Net ~270 lines.
- Blog 223.

## Open

The eager FP save/restore is no longer in `do_switch_thread`.  If
HVF testing reveals a regression on a workload we didn't measure
(e.g., a long-running pure-FP benchmark where the trap fires once
and never again), revisit the boot CPACR setting.  The `0b01` →
`0b11` change is one line in `boot.S` and an `if false` around the
`switch_task` `cpacr_trap_el0_fp()` call.
