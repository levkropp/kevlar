## Blog 215: arm64 FP-off kernel, plus the HVF `isv` crash I fell into

**Date:** 2026-04-23

Blog 213 shelved the fork_exit arc with "ASID-tagged TLBs and an
FP-off kernel rebuild remain the big unclaimed levers; neither is a
session-sized task."  The ASID lever was 340 ns per fork_exit iter
— a 0.34% win for multi-day work.  FP-off was predicted at ~2 µs
for fork_exit and "one week" of implementation because of the
dependency audit and SIMD memcpy review.

Both estimates turned out wrong.  FP-off is a half-day task because
the kernel was already FP-clean (nobody wrote `f32`/`f64` or
`#[target_feature]` anywhere in `kernel/`, `platform/`, `libs/`,
`exts/`, or `services/`), and the win is not ~2 µs on fork_exit —
it's **15-45% on essentially every syscall**, and double-digit
percentage gains on six of the seven workload benchmarks.

Along the way I hit a pre-existing bug that had silently broken
arm64 HVF since QEMU 11 landed.  The assertion was real.  The
root cause was LLVM's post-index store optimization tripping over
Apple Silicon HVF's stage-2 data-abort decoder.

## The intended change

Linux arm64 builds its kernel with `-mgeneral-regs-only` and saves
FP/NEON state only on context switch.  Kevlar did the opposite:
`+neon,+fp-armv8` in the kernel target spec, and `SAVE_FP_REGS` /
`RESTORE_FP_REGS` (528 B frame: 32× 16-byte q-regs + FPCR + FPSR)
on every EL0 exception entry and exit.  The trap.S comment claimed
the kernel was already `-neon,-fp-armv8`; that was stale — the
build spec contradicted it.

The mechanical change is short:

1. `kernel/arch/arm64/arm64.json`: features → `+v8a,+strict-align,-neon`,
   plus `"abi": "softfloat"` and `"rustc-abi": "softfloat"` because
   the default aarch64 ABI passes scalar floats in `d0` and rustc
   refuses `-neon` without a softfloat ABI.
2. `platform/arm64/trap.S`: strip `SAVE_FP_REGS` / `RESTORE_FP_REGS`
   from the user-mode sync and IRQ handlers.  `add x1, sp, #FP_FRAME_SZ`
   becomes `mov x1, sp` since PtRegs is now directly at `sp`.
3. `platform/arm64/task.rs`: add a 528-byte `FpState` (boxed), one
   per task, zero-init from every constructor.  Extend `switch_task`
   to pass `prev_fp` and `next_fp` to the assembly.
4. `platform/arm64/usermode.S`: extend `do_switch_thread` with 16×
   `stp q,q` / `ldp q,q` + FPCR/FPSR around the SP swap.  Add
   `kevlar_save_fp_to(*mut FpState)` for the fork path, which has
   to snapshot the parent's live HW FP state into the child's
   `FpState` (the trap handler no longer does that for us).

The asm files need `.arch armv8-a+fp+simd` because the surrounding
rustc target has `-neon` and the assembler (inheriting target
features via `global_asm!`) otherwise rejects q-reg operands.
That's fine: the codegen restriction is what we want; asm can
still address FP regs explicitly.

**Objdump verification.**  The whole kernel `.text` section
contains exactly three clusters of FP instructions — 48 q-pair
stp/ldp and 6 FPCR/FPSR mrs/msr — all inside `do_switch_thread`
and `kevlar_save_fp_to`, in a 304-byte address range.  Zero leaked
elsewhere.  The dependency audit (looking for anything that would
auto-vectorize or force FP codegen) was trivial because the kernel
simply never touches floats.

## The win

| benchmark        | FP-ON median | FP-OFF median | Δ      |
|------------------|-------------:|--------------:|-------:|
| `getpid`         |       80 ns  |      44 ns   | +45.0% |
| `read_null`      |      111 ns  |      63 ns   | +43.2% |
| `write_null`     |      119 ns  |      67 ns   | +43.7% |
| `statx`          |      889 ns  |     431 ns   | +51.5% |
| `mmap_fault`     |     1738 ns  |    1246 ns   | +28.3% |
| `exec_true`      |   114 µs     |    89 µs     | +22.2% |
| `fork_exit`      |   118 µs     |   107 µs     |  +9.1% |
| `sort_uniq`      |   764 ms     |   653 ms     | +14.6% |
| `tar_extract`    |   936 ms     |   628 ms     | +32.9% |
| `sed_pipeline`   |   635 µs     |   372 µs     | +41.5% |
| `pipe_pingpong`  |  2400 ns     |  2403 ns     |   0.0% |

Eight-sample medians on arm64 HVF, release, PROFILE=balanced.
Twelve more syscalls in the 15-30% range; nothing regresses past
noise once the sample count is real.

The shape is: **every EL0 exception used to move 1056 bytes of FP
state** (528 B save + 528 B restore).  Four exceptions per
fork_exit iter is 4.2 KB; most syscalls it's ~1 KB.  Remove that,
and simple syscalls — where the 1 KB was a large fraction of total
cost — get most of the 15-45% win.  Expensive syscalls (fork_exit,
exec) see the absolute savings but the proportional win is smaller
because they do so much other work.

Context switches now pay 1 KB of FP save/load (a 528 B stp q-pair
block plus FPCR/FPSR on each side).  I expected `pipe_pingpong`
(two threads ping-ponging a byte, one CS per bounce) to regress;
it lands at 0.0% median over eight samples.  The FP save fits in
L1 and the context-switch work was never dominated by that
memcpy; the save costs something like 40-50 ns per CS but that's
below the per-iter noise on pipe_pingpong.

fork_exit in quick mode went 118 → 107 µs, which is 11 µs — more
than the 2 µs blog 213 predicted.  Four syscalls per iter × ~2 µs
saved per syscall ≈ 8 µs from the trap-path cut alone, plus ~3 µs
of extra savings scattered through page-fault handlers, exec, etc.
Still 90+ µs from Linux's ~16 µs; FP-off was never going to close
that gap.  But the session-wide picture is much better than the
per-benchmark view suggests — **sort_uniq saves 112 ms per run**,
which is the kind of number that matters for real workloads.

## The unplanned find: HVF `isv` assertion

The plan called for running the full bench suite and the contract
suite under arm64 HVF to validate.  First HVF run crashed at
"page_allocator OK" with

```
Assertion failed: (isv), function hvf_handle_exception, file hvf.c, line 2181.
```

QEMU 11's HVF accelerator on Apple Silicon — upgrading from
whatever older version had worked in blog 214.  Diagnostic prints
localized it to `gic::init` step 2: the priority-register write
loop.

```rust
for i in (32..256).step_by(4) {
    mmio_write(gicd + GICD_IPRIORITYR + i, 0xa0a0a0a0);
}
```

Where `mmio_write` was the textbook `core::ptr::write_volatile`.
LLVM compiled the loop body to

```
    str     w9, [x10], #0x4     // post-index store
```

— a post-indexed 32-bit store, increments the base register after
the access.  On Apple Silicon, HVF traps this as a stage-2 data
abort, and the abort's `ESR.ISV` bit is 0 because the instruction
syndrome encoding for post-index stores is "not valid" for the
emulation path HVF uses.  HVF sees ISV=0 on a trap it was supposed
to handle and crashes:

```c
// qemu/target/arm/hvf/hvf.c:2181
case EC_DATAABORT:
    isv = syndrome & ARM_EL_ISV;
    ...
    g_assert(isv);
```

The fix is forcing the instruction encoding with inline asm so
LLVM can't pick a post-index form:

```rust
unsafe fn mmio_write(addr: usize, val: u32) {
    core::arch::asm!(
        "str {v:w}, [{a}]",
        v = in(reg) val,
        a = in(reg) addr,
        options(nostack, preserves_flags),
    );
}
```

`str w, [x]` with zero offset.  LLVM still updates the address
register separately via `add x, x, #4`, and the store itself is
always the plain form — ISV=1 under HVF.

This is a pre-existing bug, not something FP-off introduced.  The
LLVM optimizer has presumably picked post-index for GIC init since
forever.  What changed is that the QEMU HVF path tightened the
assertion in v11, and I was the first person to hit it.  The blog
214 bench numbers must have been captured on the older QEMU.  The
fix belongs on main regardless of the FP-off work.

Bonus find during the same diagnostic pass: `platform/stack_cache.rs`
had `core::arch::asm!("mov {}, rsp", ...)` — unconditional, no
`#[cfg]`.  `rsp` is x86_64; on arm64 LLVM treats it as an unknown
symbol and emits an `invalid fixup for movz/movk` error.  The
function is dead (its only caller is commented out), so the bug
never fired at runtime, but it broke the arm64 build entirely.
Arch-gated the inline asm (x86_64 `rsp`, aarch64 `sp`).

## What's next

The fork_exit gap is still ~60 µs (74 µs full-mode) vs Linux's ~16.
The 10 µs that's in `Process::fork` and the 0.3 µs in
`Process::exit` are reasonably tight now.  The remaining bulk is
the four EL0 trap round-trips and the child's on-CPU lifetime plus
the parent's `wait4` wake window.  FP-off shaved the trap round-
trips.  The next levers I want to test:

- `wait4` fast path: when the child has already exited before the
  parent calls `wait4`, avoid the wait-queue enqueue and return
  immediately.  Linux does this.  Kevlar almost certainly doesn't.
- Direct scheduler handoff on fork: the parent's `fork` syscall
  return and the child's first userland instruction don't need to
  go through a full scheduler round-trip.  Linux uses "wake and
  immediately yield" to the child.
- Syscall trap overhead: simple syscalls are down to 44 ns
  (`getpid`) but Linux is ~25 ns.  The 20 ns gap is mostly
  SAVE_REGS / RESTORE_REGS saving all 31 GPRs when the syscall
  ABI only uses 7 of them.

Those are the remaining open levers.  None of them look multi-day.
