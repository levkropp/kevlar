# Blog 095: ARM64 NEON register corruption + signal delivery fix — 101 to 114/118

**Date:** 2026-03-19
**Milestone:** M10 Alpine Linux

## Context

ARM64 contract tests had plateaued at 101/118 PASS with several stubborn
failures:  `vm.mremap_grow` (XFAIL since day one), `signals.handler_context`
(handler receives sig=0), and ~12 other tests with various silent corruptions.
All were ARM64-only; x86_64 passed clean.

---

## Bug 1: NEON register corruption across page faults (13 tests fixed)

### Symptom

`vm.mremap_grow`: mmap 1 page, memset(addr, 0xAB, 4096), mremap grow, check
data.  The check fails — every byte is 0x00.  The physical page was never
written by the user's memset, even though no SIGSEGV was raised.

### ktrace diagnosis

Built with `FEATURES=ktrace-mm` and added a Phase 3 "killer test" in mremap:
read the user VA via `copy_from_user` AND read the physical page directly.
Both returned 0x00 — the user write truly never executed (not a cache
coherency issue).

### Root cause

The ARM64 exception handler in `trap.S` only saved/restored GPRs (x0-x30):

```asm
.macro SAVE_REGS
    sub sp, sp, #(34 * 8)
    stp x0, x1, [sp, #0]
    ...  // x0-x30, sp_el0, elr_el1, spsr_el1
.endm
```

But the kernel target spec had `+neon,+fp-armv8`, meaning the kernel freely
used NEON registers (v0-v31).  musl's ARM64 memset uses NEON for bulk fills:

```asm
dup  v0.16b, w1      // splat fill byte into 128-bit register
stp  q0, q0, [x0]    // store 32 bytes per iteration
```

When the first store faults (demand page), the kernel page fault handler runs
compiled Rust code that clobbers v0.  After ERET, memset stores whatever
garbage the kernel left in v0 — zeroes in this case.

This affected ANY test where user code used NEON across a page fault or
syscall: memset, memcpy, string operations, printf formatting.  The 13 tests
that "magically" started passing were all victims of silent NEON corruption.

### Fix

Added `SAVE_FP_REGS` / `RESTORE_FP_REGS` macros to `trap.S` for user-mode
exceptions (lower EL sync + IRQ).  Saves v0-v31 + FPCR + FPSR = 528 bytes:

```asm
.macro SAVE_FP_REGS
    sub     sp, sp, #528
    stp     q0,  q1,  [sp, #0]
    stp     q2,  q3,  [sp, #32]
    ...
    stp     q30, q31, [sp, #480]
    mrs     x0, fpcr
    mrs     x1, fpsr
    str     x0, [sp, #512]
    str     x1, [sp, #520]
.endm
```

Kernel-mode exceptions (`handle_curr_spx_*`) don't need FP save because the
kernel's own calling convention preserves callee-saved registers, and the
kernel never returns to user mode from those handlers.

Note: disabling NEON via `-neon,-fp-armv8` in the target spec was attempted
first but fails — NEON is mandatory for the AArch64 ABI.

---

## Bug 2: Signal handler receives sig=0 (ARM64 only)

### Symptom

`signals.handler_context`: install handler for SIGUSR2, `kill(getpid(), 12)`,
check `received_signal`.  Handler always receives 0 instead of 12.

### ktrace diagnosis

Added `SIGNAL_SEND`, `SIGNAL_CHECK`, and `SIGNAL_DELIVER` ktrace events
(event types 20-22) to trace the full signal path.  Built with
`FEATURES=ktrace-mm,ktrace-syscall`.

The trace revealed:

```
SYSCALL_ENTER  kill(pid=1, sig=12)
SIGNAL_SEND    pid=1 sig=12 action=Handler handler=0x400450
SYSCALL_EXIT   kill → 0
SIGNAL_DELIVER sig=12 regs[0]=12 pc=0x400450 x30=0x402e70
SYSCALL_ENTER  rt_sigreturn
```

The signal WAS delivered (rt_sigreturn proves the handler ran), and
`SIGNAL_DELIVER` confirmed `frame.regs[0]=12` after `setup_signal_stack`.
But the handler received x0=0.

### Root cause

Double-write to `frame.regs[0]` in `arm64_handle_exception`:

```rust
EC_SVC_A64 => {
    let ret = arm64_handle_syscall(frame);      // dispatches kill
    unsafe { (*frame).regs[0] = ret as u64; }   // OVERWRITES signal!
}
```

The syscall dispatch already writes the return value to `frame.regs[0]` AND
delivers pending signals (which overwrites `regs[0]` with the signal number).
But then `arm64_handle_exception` blindly overwrites `regs[0]` with the
syscall return value (0 for kill), destroying the signal number.

This bug was invisible on x86_64 because the x86_64 interrupt handler doesn't
have this redundant write — signal delivery is the last thing to touch the
frame before IRET.

### Fix

One-line removal:

```rust
EC_SVC_A64 => {
    // The dispatch writes regs[0] and handles signal delivery.
    // Do NOT overwrite regs[0] — it would clobber the signal number.
    super::syscall::arm64_handle_syscall(frame);
}
```

---

## Additional fix: DSB after intermediate page table writes

Added `dsb ishst` barriers in `traverse()` and `traverse_to_pt()` after
writing intermediate table descriptors (PGD→PUD→PMD).  The final PTE write
already had DSB, but intermediate levels did not.  While this alone didn't
fix the mremap_grow issue (the NEON corruption was the real cause), it's
architecturally correct — the hardware page table walker needs these stores
to be visible before descending to the next level.

---

## Results

### Contract tests

| Arch   | Before | After  | XFAIL | FAIL |
|--------|--------|--------|-------|------|
| ARM64  | 101/118 | **114/118** | 4 | 0 |
| x86_64 | 116/118 | 116/118 | 2 | 0 |

13 ARM64 tests fixed by NEON save/restore, 1 by signal delivery fix.
Cleaned `known-divergences.json` from 19 entries down to 6.

### Benchmarks (x86_64 KVM, Kevlar vs Linux)

No regressions from these ARM64-only changes (as expected — x86_64 code
paths untouched):

| Benchmark | Linux | Kevlar | Ratio |
|-----------|-------|--------|-------|
| gettid | 90ns | 1ns | 0.01x |
| mmap_fault | 1.6us | 13ns | 0.01x |
| mmap_munmap | 1.3us | 361ns | 0.28x |
| signal_delivery | 1.1us | 512ns | 0.47x |
| sched_yield | 147ns | 73ns | 0.50x |
| getpid | 90ns | 62ns | 0.69x |

**Summary: 29 faster, 13 OK, 2 marginal, 0 regression** vs fresh Linux KVM.
Down from 41 faster against stored baseline — investigating individual
benchmark movements next.

---

## Files changed

- `platform/arm64/trap.S` — SAVE_FP_REGS/RESTORE_FP_REGS for user exceptions
- `platform/arm64/interrupt.rs` — removed redundant regs[0] overwrite in SVC
- `platform/arm64/paging.rs` — DSB in traverse() after intermediate table writes
- `kernel/debug/ktrace.rs` — SIGNAL_SEND/CHECK/DELIVER event types (20-22)
- `kernel/process/process.rs` — ktrace signal instrumentation
- `kernel/syscalls/mremap.rs` — Phase 3 Method B diagnostic (ktrace-mm only)
- `testing/contracts/known-divergences.json` — pruned from 19 to 6 entries
