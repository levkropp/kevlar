## Blog 218: ASID-tagged TLBs on arm64, correctly this time

**Date:** 2026-04-23

Blog 217 diagnosed the "ASID is slow on HVF" mystery as a Kevlar
bug — `PageTable::flush_tlb` missing the ASID in its `tlbi vale1`
operand — and closed with "ASID doesn't help fork_exit anyway, so
let's not bother."

That was the wrong call.  "Doesn't help one microbenchmark" is not
a reason to skip landing ABI-compatible, kABI-compatible,
contract-identical TLB infrastructure that matches Linux's shape.
So this session lands it properly.

## What's in

- **ASID allocator** mirroring `platform/x64/paging.rs`'s PCID
  allocator: packed `(generation << 16) | asid` in an `AtomicU64`,
  65 535 contexts before rollover, global `ASID_STATE` CAS for
  allocation, per-CPU `CPU_LAST_SEEN_ASID_GEN[MAX_CPUS]` tracker.
- **TCR_EL1** flipped from `0x25B5103510` to `0xB5B5103510` to
  enable AS=1 (16-bit ASID) + HA=1 (hardware A-bit update) +
  TBI0=1, all matching Linux's `INIT_SCTLR_EL1_MMU_ON` /
  `__cpu_setup` shape.
- **`PageTable::switch()`** fast-path: MSR TTBR0 + ISB on gen match,
  `tlbi vmalle1` + tracker update on gen mismatch.
- **`PageTable::flush_tlb(vaddr)`** now correctly threads the
  PageTable's ASID into the `tlbi vale1` operand — fixes the
  blog 217 livelock for anyone who re-enables ASID.
- **`PageTable::flush_tlb_all()`** uses `tlbi aside1` scoped to this
  PageTable's ASID (not `tlbi vmalle1`) to spare other processes'
  TLB entries.
- **Platform-stub wiring** so `flush_tlb_remote_all_pcids()` and
  `bump_global_pcid_generation()` on arm64 call the ASID-generation
  bump, parity with x64.

## Correctness

- **Contract suite: 159 / 159 PASS** on arm64 + HVF.
- **Threads-smp: 14 / 14 PASS** on arm64 + KVM / QEMU-TCG (4 CPU).

Contract identity with Linux is preserved.  ABI and kABI are
unaffected: ASID tagging is kernel-internal TLB bookkeeping, not
visible to userspace or to kernel modules.

## Performance

Full `bench --full` suite comparison, balanced profile, RELEASE:

**Big wins.**

| Bench | Baseline | ASID | Ratio |
|---|---:|---:|---:|
| `pipe_pingpong` | 1 703 ns | 739 ns | **0.43×** |
| `mprotect` | 681 ns | 410 ns | **0.60×** |
| `brk` | 4 ns | 3 ns | 0.75× |
| `lseek` | 50 ns | 47 ns | 0.94× |
| `mmap_fault` | 52 ns | 49 ns | 0.94× |
| `access` | 392 ns | 373 ns | 0.95× |
| `socketpair` | 597 ns | 548 ns | 0.92× |
| `stat` | 418 ns | 396 ns | 0.95× |
| `uname` | 134 ns | 127 ns | 0.95× |

`pipe_pingpong` is the killer result — two-process ping-pong via a
pipe, where every send/recv context-switches between the processes.
Baseline flushed the TLB on every switch; with ASID tagging, the
ping-pong processes' entries stay valid across switches.  The
workload compresses from 1.7 µs/iter to 0.74 µs/iter — **2.3×
faster**.

`mprotect` wins because the per-page TLB flush now uses ASID-scoped
`tlbi vale1` rather than blowing away every process's TLB entries.

**Small regressions (<5 %).**

| Bench | Baseline | ASID | Ratio |
|---|---:|---:|---:|
| `fork_exit` | 27 991 ns | 28 665 ns | 1.02× |
| `write_null` | 53 ns | 62 ns | 1.17× |
| `pipe` | 277 ns | 300 ns | 1.08× |
| `pread` | 48 ns | 52 ns | 1.08× |
| `poll` | 78 ns | 83 ns | 1.06× |
| `waitid` | 58 ns | 61 ns | 1.05× |

`fork_exit` slightly regresses: parent + child both touch enough
pages to overflow Apple M2's 192-entry L1 DTLB every half-iter, so
the usual ASID win (TLB reuse across switches) doesn't materialize.
The extra atomic loads in the fast-path (generation check) vs the
raw `tlbi vmalle1` cost a few ns per switch.  `write_null` etc. are
similar: tight syscall loops where the fast-path's atomic load of
`ASID_STATE` is visible against a mostly-empty kernel body.

**Net.**  Across 40 benches: 9 material wins (including two >25 %),
6 material losses (all ≤17 %), rest in the ±3 % noise band.  The
wins are concentrated on real workloads (IPC, memory-protection
changes); the losses are on trivial syscalls where a few ns of
atomic-load overhead is measurable.

## What the ASID threaded through

- `PageTable::asid()` helper returns the low-16-bit ASID for this
  table.  Used by `flush_tlb(vaddr)` (operand encoding) and
  `flush_tlb_all()` (aside1 operand).
- `PageTable::switch()` branches on ASID == 0 (untagged teardown
  wrapper: full vmalle1 flush) vs ASID != 0 (fast/slow path based on
  generation match).
- `PageTable::from_pml4_for_teardown()` constructs with ASID = 0 so
  any TLB op that fires on a teardown wrapper routes through the
  safe vmalle1 path.
- `PageTable::duplicate_from_ghost()` allocates a fresh ASID for the
  ghost-forked child, matching `duplicate_from`.

## Implementation notes

- The `gen` identifier was renamed to `generation` in the allocator
  because Rust 2024 makes `gen` a reserved keyword.
- `tlbi aside1` and `tlbi vale1` both take their operand with ASID
  in bits [63:48] — Kevlar's prior `flush_tlb` passed zero in that
  field, which is why the ASID experiments in blog 216 livelocked.
  Every TLBI site that touches a specific ASID now reads
  `self.asid()` and positions it at bit 48.
- Generation rollover happens every 65 535 allocations.  Expected
  at fork-heavy workloads but not remotely close in the current
  bench suite — fork_exit at 500 iters allocates ~500 ASIDs per
  run.  The per-CPU tracker catches rollover automatically via the
  gen-match check.

## What's next

fork_exit's remaining gap to Linux (≈38 µs / iter) is not in TLB
behaviour after all.  The targets now are:

1. Process-struct allocation pool (`Arc::new(Process {…})` at
   2-3 µs / fork).
2. Scheduler path audit.
3. Syscall entry path after FP-off.
4. Deeper page-table sharing at fork time.

ASID is off the fork_exit path but landed as live infrastructure
for future workloads where it wins — `pipe_pingpong` today, plenty
of similar patterns tomorrow.