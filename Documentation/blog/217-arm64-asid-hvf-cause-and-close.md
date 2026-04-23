## Blog 217: the arm64 ASID / HVF mystery, solved and closed

**Date:** 2026-04-23

Blog 216 left the ASID fast-path on arm64 as an open mystery: every
attempt at enabling it regressed fork_exit by ~200×, and the likely
cause was vaguely "HVF is slow for TTBR writes with non-zero ASID
bits."  This session nailed it.

**TL;DR.**  The bug was a latent one in `platform/arm64/paging.rs`:
`PageTable::flush_tlb(vaddr)` issued `tlbi vale1, {op}` with `op = VA
>> 12` — correct VA in bits [43:0], but **zero** in the ASID field
[63:48].  With ASID=0 (our previous baseline) that silently worked.
With non-zero ASID in TTBR0, the TLBI invalidated no entries for the
running process — CoW / permission-fault fixes stayed invisible to
the TLB, user retried the same fault, kernel re-fixed the same PTE,
retry, fault, retry, fault.  Livelock at roughly 10 000 fault
iterations per fork_exit cycle.

HVF was never the problem.  Kevlar's TLBI operand was.

## How it was found

Blog 216's tooling already measured `msr ttbr0_el1` at **0-1 cntvct
ticks** (< 41 ns) regardless of ASID content — so the "5-8 ms per
MSR" headline was an arithmetic artifact from dividing a real
per-iter regression by the MSR count.  The cost was real but pinned
on the wrong instruction.

This session's first step was boundary instrumentation: added cycle
timing around `eret → user → SVC` and `trap entry → eret`, dumped
buckets.  All three buckets — user-space window, kernel-side
per-trap, full round-trip — came back clean: <10 µs across the full
fork_exit cycle, even with ASID=1 in TTBR0.  Yet the bench still
reported 11-12 ms/iter.

That forced a counter on exception-class dispatch.  Smoking gun:

```
EC-COUNT total=5000000 ec[21]=4882 ec[32]=41 ec[36]=4970691 ec[37]=24386
```

EC=21 is SVC (syscall) — 4 882 of them, expected.  EC=36 is **data
abort from lower EL (user-space)** — _4.97 million_ in a 500-iter
bench.  9940 data aborts per fork_exit iteration.  For a child that
just calls `_exit(0)`.

Dumping FAR on the first 30 faults showed the pattern:

```
FLT #26 far=0x9ffffed80 esr=0x9200004f dfsc=0xf write=true
FLT #27 far=0x9ffffec30 esr=0x9200004f dfsc=0xf write=true
FLT #28 far=0x9ffffec30 esr=0x9200004f dfsc=0xf write=true
FLT #29 far=0x9ffffec30 esr=0x9200004f dfsc=0xf write=true
FLT #30 far=0x9ffffec30 esr=0x9200004f dfsc=0xf write=true
```

DFSC=0xf = permission fault at level 3 (the leaf PTE).  The same
address faults four times in a row.  That's the livelock.

## Why ASID=0 hides it

`tlbi vale1, xT` requires its operand packed as:

- bits [43:0] — VA[55:12] (the 4K-aligned page)
- bits [63:48] — ASID

When the operand has ASID=0, `tlbi vale1` invalidates only entries
tagged with ASID=0.  Global (nG=0) kernel entries are ignored.
Non-global entries tagged with a different ASID are ignored too.

Before this session, Kevlar's TTBR0 writes never set the ASID bits
— the kernel ran with TCR.AS=0 and all user PTEs were effectively
global (we hadn't yet set `ATTR_NG` either).  Everything was
ASID=0, `tlbi vale1 VA >> 12` flushed the one and only tag space,
and the world was consistent.

Blog 216's nG-bit fix (`arm64/paging: set nG (non-Global) on user
PTEs`) plus the ASID experiments flipped both preconditions: user
PTEs became ASID-tagged, and we started writing non-zero ASID values
to TTBR0.  The `tlbi vale1` operand in `flush_tlb()` still zeroed
the ASID field, which now meant "invalidate no entries for this
process's running ASID."  Cold case, no diagnostic, wrong answer.

## The fix

Thread the live ASID through:

```rust
let addr = ((vaddr.value() >> 12) as u64) | (self.asid() << 48);
core::arch::asm!("tlbi vale1, {}", "dsb ish", "isb", in(reg) addr);
```

With the fix + ASID=1 in TTBR0 + `tlbi vmalle1` per switch:
fork_exit = **52.7 µs/iter**, better than the 53.5 µs baseline by a
hair.  EC=36 counts drop from 5 M back to ~1 400 (normal demand
paging).  Livelock gone.

## Does ASID actually help fork_exit?

With the bug fixed we can properly evaluate the original lever.
Stack the full fast-path: per-PageTable ASID allocator (packed
generation | asid like x64's PCID), `CPU_LAST_SEEN_ASID_GEN[cpu]`
tracker, TCR.AS=1, TCR.HA=1, `PageTable::switch` skipping TLBI on
gen-match.  14/14 threads-smp passes.

fork_exit numbers over 7 runs, RELEASE+balanced:

| Config                        | fork_exit/iter |
|------------------------------|---------------:|
| Baseline (tlbi vmalle1 every switch) | 53.1-54.3 µs |
| ASID fast-path (no TLBI on fast path) | 57.6-59.0 µs |

The ASID fast-path is **~4 µs per iter slower** than the baseline it
was supposed to replace.

Why?  Apple M2's L1 DTLB is 192 entries.  fork_exit alternates two
processes (parent + newly-forked child) that each touch ~100-200
pages during their half of the cycle.  On every switch, whichever
process runs next has to refill its TLB from memory either way:

- Baseline: `tlbi vmalle1` empties the TLB on switch → next
  process's accesses all miss → refill.
- ASID: preserve both ASIDs' entries in the TLB → but they overflow
  the 192-entry L1 immediately → LRU evicts → effectively the same
  refill cost.

Plus the fast-path does four atomic loads + a compare before the MSR
(vs baseline's raw TLBI + DSB + ISB), and those add overhead.
Without the TLB-reuse win from preserving entries across switches,
the extra branch work is pure cost.

**So: on this workload, on this hypervisor, ASID-tagged TLBs are a
wash-to-regression.  Blog 213's hypothesis ("skip tlbi vmalle1 and
save milliseconds") was wrong.  The tlbi was never the bottleneck.**

## What landed today

- **`flush_tlb` ASID-operand fix (comment-only commit for now)**.  No
  functional change because we still run with ASID=0 everywhere, but
  the inline comment documents the bug so the next person who
  enables ASID tagging doesn't rediscover the livelock.  Actual
  ASID-threading in `flush_tlb` waits for the feature to actually
  ship.
- **Closed investigation doc**.  Written up in
  `Documentation/optimization/asid-hvf-investigation.md` with the
  bit-bisect table, the measurements, and the closing note that
  ASID is not the right lever on fork_exit at current TLB sizes.

## What I didn't commit

- The ASID allocator, TCR.AS=1, per-CPU generation tracker, and
  fast-path switch — working code but no perf win on fork_exit.
  Kept as a patch in the session log in case a future workload
  with more TLB-persistent state (long-running user jobs doing
  short context-switch bursts) reverses the picture.
- The Linux-side `cpu_do_switch_mm` instrumentation we wanted for a
  side-by-side histogram.  The natively-on-macOS Linux build is a
  header whack-a-mole dead end; a future session with a Linux VM
  will pick this up if the evidence demands it.

## What's next for fork_exit

The remaining gap to Linux (15 µs/iter) is 38 µs per iter, and it's
now clear the gap doesn't live in TLB behaviour.  Candidates in
rough order:

1. **Process-struct pool**: `Arc::new(Process {…})` is 2-3 µs/fork.
2. **Scheduler path audit**: `process::switch` at 3 µs/iter; saw in
   blog 215 that trimming FP/NEON save helped, more to find.
3. **Syscall trap overhead**: SVC → `arm64_handle_syscall` chain,
   after FP-off already pruned.
4. **Fork's page-table duplicate**: already at ~5.8 µs after blog
   212's lazy leaf-PT CoW; hard to compress further without sharing
   even more.

Each of those is a direct target.  The ASID thread is closed.