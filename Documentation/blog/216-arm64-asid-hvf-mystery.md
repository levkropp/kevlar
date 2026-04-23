## Blog 216: the arm64 ASID / HVF mystery

**Date:** 2026-04-23

Blog 215 landed FP-off and set up a clean platform for the last big
unclaimed fork_exit lever: ASID-tagged TLBs.  Today's session spent
itself against that lever and came back without the win, but with a
concrete and reproducible observation that I couldn't have predicted
from reading Linux's source alone:

**Writing any TTBRx_EL1 with non-zero ASID bits [63:48] costs ~5–8 ms
per MSR on HVF/Apple Silicon in our kernel.**  On the same host, the
same QEMU, the same HVF build, running a stock Linux kernel, the
equivalent `cpu_do_switch_mm` sequence completes at hardware speed —
Linux's fork_exit on HVF clocks 15.4 µs/iter.  Ours, once ASID-tagged,
goes to 11–17 ms/iter.

What's in this post: the finding, the four isolation experiments that
forced me to believe it, and a set of concrete next steps.

## The plan going in

Mirror x86_64's PCID scheme on arm64:

- 16-bit ASID (TCR.AS=1, 65536 contexts, rollovers never fire on our
  fork_exit micro).
- Per-PageTable `AtomicU64` packed as `(generation << 16) | asid`.
- Per-CPU `CPU_LAST_SEEN_ASID_GEN[MAX_CPUS]` tracker.
- `PageTable::switch()` fast path: if `cpu_last_seen == global_gen`,
  just write TTBR0 + ISB — no TLBI.  Slow path (gen bump): `tlbi
  vmalle1` once, update tracker, then write.

The x64 version of this has been in production since blog 203 and is
well-tested.  The arm64 port is greenfield — arm64/paging.rs had no
ASID code at all — but the structural translation is mechanical.

## The expected win

Blog 213 had measured the per-switch `tlbi vmalle1 + dsb ish + isb` at
340 ns/iter in a micro-benchmark (a cold measurement in isolation),
but suspected the composite per-switch trap cost on HVF was
multi-µs.  Removing TLBI from the common case was the last big lever
on fork_exit at the time.  Conservatively: 2–5 µs/iter off the
current 58 µs median.  Target: ≤ 54 µs.

## The nG-bit prerequisite

One thing the plan had right: arm64 user PTEs *must* carry the nG
(non-Global) bit for ASID tagging to do anything at all.  Linux sets
`PTE_NG` on every user prot macro:

```c
#define _PAGE_SHARED   (_PAGE_DEFAULT | PTE_USER | PTE_RDONLY | PTE_NG | ...)
#define _PAGE_READONLY (_PAGE_DEFAULT | PTE_USER | PTE_RDONLY | PTE_NG | ...)
```

Kevlar never did.  The bug was harmless at rest because
`PageTable::switch()` issues `tlbi vmalle1` on every context switch,
which wipes global entries too.  The moment you switch to ASID
fast-path (no TLBI per switch), a global user-PTE entry from process
A stays valid under process B's ASID — so process B transparently
reads process A's memory.  Complete correctness corruption.

This was the first correctness fix, and it lands standalone in the
commit just before this post: `arm64/paging: set nG (non-Global) on
user PTEs`.  14/14 threads-smp, fork_exit unchanged at ~59 µs.  This
fix is valid regardless of whether we ever get the ASID fast-path to
pay off — user PTEs were always semantically non-global.

## First attempt: set TCR.AS=1, write TTBR0 with ASID

With nG in place, I built out the ASID allocator (structural mirror
of `platform/x64/paging.rs`), added `MAX_CPUS = 8` to
`platform/arm64/smp.rs`, and flipped TCR_EL1 from `0x25B5103510` to
`0xB5B5103510`.  That adds TCR.AS=1 (16-bit ASID) and TCR.HA=1
(hardware Access-Flag update, which Linux unconditionally enables on
Apple Silicon).  Left TCR.A1=0 so that ASID lives in TTBR0 and the
switch path is a single MSR.

Fast path:

```rust
let ttbr0 = (self.pgd.value() as u64) | (asid << 48);
asm!("msr ttbr0_el1, {t0}", "isb", t0 = in(reg) ttbr0);
```

Boot smoke + 14/14 threads-smp still passed.  fork_exit: **11–12
ms/iter**.  200× regression.

## Second attempt: Linux's exact sequence, A1=1

OK so the single-MSR approach is broken — either the ASID bits in
TTBR0 are the problem or Linux's specific three-step dance is
required for correctness.  Set TCR.A1=1 (ASID source = TTBR1), added
a reserved pg-dir (zero page allocated at paging init) to park TTBR0
during the transition, mirrored Linux's `cpu_do_switch_mm`:

```rust
asm!(
    "msr ttbr0_el1, {res}",   // park TTBR0 at reserved (empty) pg-dir
    "msr ttbr1_el1, {t1}",    // kernel pgd | (asid << 48)
    "msr ttbr0_el1, {t0}",    // new user pgd (zero ASID bits)
    "isb",
    ...
);
```

This is bit-for-bit `arch/arm64/mm/context.c::cpu_do_switch_mm`.
Added the CnP bit on TTBR0 for non-zero ASID (Apple Silicon
advertises CnP; Linux sets it; I set it).  Boot smoke passed, threads
passed.  fork_exit: **16.6 ms/iter**.  Still catastrophically slow.

## Third attempt: what if I put the ASID bits on TTBR1 only?

Linux writes ASID to TTBR1, not TTBR0 (because TCR.A1=1).  Maybe
HVF's pathology is specifically on TTBR0-with-ASID — if so, writing
ASID to TTBR1 alone (with TTBR0 clean) should escape it.

Already covered by attempt 2 above, which *also* writes TTBR0 with
zero ASID bits in the final MSR.  Still 16.6 ms.  Therefore the
pathology is not TTBR0-specific.

## Fourth attempt: the isolation experiment

The key diagnostic: keep everything in attempt 1 — allocator, packed
atomic, per-CPU tracker, fast/slow paths, counters, the whole
structural mirror — but **don't OR the ASID bits into TTBR0**.  Write
the user pgd with zero ASID bits:

```rust
let ttbr0 = self.pgd.value() as u64;   // deliberately no asid<<48
let _ = asid;
```

Everything else identical.  If the slowdown is in the tracker, the
allocator, the branchy control flow, or anywhere in our code, it'll
still be slow.  If the slowdown is specifically in "writing non-zero
ASID bits to TTBR," this will be fast.

**Result: 63 µs/iter.**  Back to baseline (58 µs) within noise.

That's the smoking gun.  Writing TTBR0 with non-zero bits [63:48] on
HVF costs us ~5–8 ms per MSR.  Writing TTBR1 with non-zero bits
[63:48] does the same.  Both are orders of magnitude slower than the
same MSR with zero top bits.

## What the counters say

I added `SWITCH_FAST_HITS` / `SWITCH_SLOW_HITS` / `SWITCH_ASID0_HITS`
atomics to confirm the path.  Over 500 fork_exit iterations the
fork_exit bench shows:

```
ASID-DIAG fast=100  slow=0 asid0=0
ASID-DIAG fast=1000 slow=0 asid0=0
BENCH fork_exit 500 5985493542 11970987
```

1000 fast-path hits, zero slow-path firings, zero kernel-ASID
fall-throughs.  The 5.99 s total over 1000 switches is **5.99 ms per
switch** of ASID-MSR overhead.  The tracker works perfectly; the
hardware (or HVF) is what costs.

## What this rules out

- Not a slow-path bug: slow path never fires in the bench.
- Not the tracker or the allocator: the isolation experiment proves
  they're free.
- Not the three-MSR Linux dance vs single-MSR: both regress identically
  per ASID-bearing write.
- Not CnP: enabling the CnP bit (which Linux sets on Apple Silicon)
  doesn't change the outcome.
- Not fast-vs-slow path selection, not nG placement, not the number of
  ISBs, not the DSB scope (`ish` vs `sy`).

It's the ASID bits themselves, in any TTBR.

## What I can't explain

Linux, running on the same QEMU-HVF setup (verified during the
previous session with a cross-compiled bench binary and a minimal
initramfs), achieves 15.4 µs/iter on fork_exit.  Linux's
`cpu_do_switch_mm` does exactly what I did in attempt 2: reserved
TTBR0 parking, TTBR1 with ASID bits, TTBR0 with user pgd, single
ISB.  On the same hypervisor, at the same EL, through the same QEMU
front-end.  Linux has the same pathology or it doesn't — and it
clearly doesn't, or fork_exit would be ~30 ms rather than 15.4 µs.

I don't have a theory I'm willing to defend.  The next session needs
real runtime evidence — a side-by-side snapshot of SCTLR, TCR, TCR2,
ID_AA64MMFR\*, PSTATE, CPACR and other control registers on both
kernels at the point where `cpu_do_switch_mm` is called.  Until then
the plan is retired.

## The Kevlar-specific candidate list

In rough order of "smells like it could matter":

1. **SCTLR_EL1 bits we don't set that Linux does**: SA (bit 3), SA0
   (bit 4), IESB (bit 21), EIS (bit 22), EOS (bit 11), ITFSB (bit
   37), SPAN (bit 23).  If any of these changes HVF's trap-emulate
   handling for TTBRx writes, we'd see this shape.
2. **TCR2_EL1** (FEAT_TCRX): Linux writes it conditionally; we don't
   touch it.  If HVF treats an unconfigured TCR2 as "assume worst
   case" during TTBR-with-ASID writes, we'd trap where Linux doesn't.
3. **CONTEXTIDR_EL1**: Linux optionally writes it with the task PID
   on every context switch (`CONFIG_PID_IN_CONTEXTIDR`).  We never
   touch it.  Unclear whether HVF keys any ASID-related shadow off
   CONTEXTIDR.
4. **Some Apple-Silicon-specific register we're missing entirely**.
   M-series ARM cores expose a pile of implementation-defined system
   registers; Linux configures some of them via early CPU errata /
   feature paths we don't run.

## What landed today

- **nG-bit user-PTE fix** (commit `arm64/paging: set nG (non-Global)
  on user PTEs`): pure correctness cleanup.  User PTEs now carry bit
  11 of the descriptor.  Baseline perf unchanged (59 µs median).
  Valid regardless of whether ASID ever pays off.
- Four isolation experiments, deleted at the end of the session
  rather than committed — they lived only to produce evidence.

## What comes next

- **Rebuild Linux for arm64 with a small init and ktrace-style
  instrumentation in `cpu_do_switch_mm`**, boot it on the same QEMU
  HVF setup, compare its per-MSR cost to ours.
- **Runtime register-state diff**: dump SCTLR/TCR/TCR2/CONTEXTIDR/
  ID_AA64MMFR\* on both kernels just before `PageTable::switch()` /
  `cpu_do_switch_mm`, compare byte-for-byte.
- **In-kernel ASID micro-benchmark**: write TTBR0 with non-zero ASID
  bits in a tight 10k-iter loop from a kernel thread on both
  kernels, measure `cntvct_el0` delta.  That removes the fork/exit
  path entirely and isolates the MSR cost.
- **Minimal bare-metal repro**: build a standalone 100-line bare-metal
  binary that configures TCR.AS=1 + nG user PTEs and loops
  TTBR-with-ASID writes.  If the pathology reproduces there, it's
  nothing specific to our kernel's shape — it's the minimum set of
  ARM state that triggers it.

Planning this out comprehensively is the next post.
