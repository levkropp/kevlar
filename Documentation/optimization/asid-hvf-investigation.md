# ASID / HVF Investigation Plan

**Status:** open, see blog 216 for the prior session's findings.
**Goal:** explain why `cpu_do_switch_mm` runs at hardware speed on Linux/HVF/Apple
Silicon but costs ~5–8 ms per TTBR-with-ASID MSR in Kevlar on the same host.

## Hypotheses (in the order we will test them)

Short list of things that could plausibly explain why Kevlar pays a huge per-MSR
cost and Linux doesn't.  The whole plan is structured around knocking these down
or promoting them:

H1. **HVF traps TTBR writes-with-ASID and the host-side handler is cheap iff
    the guest's control-register state is in a specific shape.**  Something we
    don't set in SCTLR / TCR / TCR2 / CPACR puts HVF onto a slow emulation path.
H2. **A missing side-effect write that Linux issues alongside the MSR
    (e.g. CONTEXTIDR_EL1, `post_ttbr_update_workaround`'s icache flush on
    specific errata).**  Removing this side-effect on Linux would also show
    the 5 ms cost.
H3. **An Apple-Silicon-specific register or implementation-defined bit that
    Linux's errata path sets and we don't**, causing HVF to run ASID MSRs on
    a cold/slow path.
H4. **Our kernel's overall TLB / page-table state (e.g. kernel TTBR1 layout,
    identity map reuse, MAIR config) provokes HVF into per-MSR
    revalidation** that Linux's layout avoids.
H5. **The cost isn't per-MSR — it's per *something triggered* by the MSR**
    (e.g. stage-2 cache invalidation, HVF's nested-TLB shadow rebuild) and
    Linux avoids triggering it because of something further up the
    context-switch call chain.

## Prerequisites

The investigation needs a **reproducible Linux-on-HVF harness** that mirrors
our Kevlar harness byte-for-byte.  Today's "Linux does 15.4 µs" number came
from an ad-hoc cross-compile of `bench.c` and a minimal initramfs and isn't
reliably reproducible.  Fix that first.

### P1. Build our own arm64 Linux distro

Why: every experiment after this (state dump, instrumentation, bisect) needs to
modify Linux and reboot.  A stable minimal build is table stakes.

- Pin Linux to a **stable release** (6.11 LTS is probably right — recent
  enough for all relevant arm64 features, but not in-development).  Record
  the SHA in this doc.
- Config: start from `defconfig`, strip to the minimum that boots on
  QEMU virt + HVF.  Aim for <5 MB compressed Image.
- Rootfs: `busybox` + our cross-compiled `bench`, packed as cpio initramfs.
- Boot with **exactly the same** QEMU invocation we use for Kevlar:
  `-machine virt,accel=hvf -cpu host -m 1024 -mem-prealloc -kernel Image
  -initrd initramfs.cpio -append "init=/bin/bench …"`.
- Capture `dmesg` to confirm HVF enables ASID (`ASID allocator initialised
  with 65536 entries`), CnP, E0PD, HA — the features we saw in the
  previous session.
- Commit the build harness (Makefile target) to `tools/linux-on-hvf/` so
  anyone can reproduce.

**Exit criterion:** `make linux-on-hvf-bench ARGS="fork_exit"` prints a
`BENCH fork_exit 500 …` line matching (± 10%) the 15.4 µs/iter target.  Once
we can reproduce, we can instrument.

### P2. Baseline Linux numbers

Run the full bench suite (not just fork_exit) on Linux-on-HVF.  Blog 214
listed the one-shot Linux numbers used for our ratios — refresh them all at
once here so every comparison in this investigation uses the same Linux
baseline.  Save to `/tmp/linux-on-hvf-bench.log`.

## Phase 1: Runtime state diff

The fastest way to find a cause is to dump the guest's control-register state
on both kernels at the same logical point and diff.

### 1A. Dump harness on Kevlar

Add a `dump_arch_state(&str)` helper in `platform/arm64/mod.rs` that prints,
in a stable one-line-per-register format:

```
KVR-STATE label=<label> cpu=<id>
  SCTLR_EL1  = 0x...
  TCR_EL1    = 0x...
  TCR2_EL1   = 0x... (if FEAT_TCRX)
  TTBR0_EL1  = 0x...
  TTBR1_EL1  = 0x...
  MAIR_EL1   = 0x...
  CPACR_EL1  = 0x...
  CONTEXTIDR_EL1 = 0x...
  CNTKCTL_EL1 = 0x...
  SPSR_EL1   = 0x...
  ID_AA64MMFR0_EL1 = 0x...
  ID_AA64MMFR1_EL1 = 0x...
  ID_AA64MMFR2_EL1 = 0x...
  ID_AA64MMFR3_EL1 = 0x...
  ID_AA64PFR0_EL1 = 0x...
  ID_AA64ISAR*_EL1 = 0x...
  MIDR_EL1    = 0x...
  REVIDR_EL1  = 0x...
```

Call sites:
- `bsp_early_init` right before `boot_kernel()`
- First entry to `PageTable::switch()` (one-shot static flag)

### 1B. Dump harness on Linux

Patch `arch/arm64/mm/context.c::cpu_do_switch_mm` to emit the same format
via `pr_err` once.  Also dump at `arch/arm64/mm/init.c::mem_init` end-of-boot
so we have a pre-fork snapshot too.

### 1C. Diff

`diff` the two snapshots.  Expected: a bounded set of register-bit
differences.  Each one is a hypothesis.

**Exit criterion:** have a written list of *every* observable register-bit
difference between the two kernels at the same logical point.

## Phase 2: Bit-bisection

For each register-bit difference from Phase 1C:

1. Make the change in Kevlar (either set or clear the bit to match Linux).
2. Rebuild, boot, run fork_exit.
3. If switching one bit reduces the ASID cost substantially, promote it.
4. If toggling nothing helps, flip *all* the diff bits at once to confirm
   the hypothesis that differing state is the cause.

Track results in a table in this doc:

| Register | Bit(s) | Linux | Kevlar | Flipped in Kevlar | fork_exit result |
|----------|--------|-------|--------|-------------------|------------------|
| ...      | ...    | ...   | ...    | ...               | ...              |

## Phase 3: Micro-benchmark the MSR cost in isolation

The fork_exit bench includes fork, exec-free child, scheduler, wait4 —
~58 µs of not-ASID work per iter.  That dwarfs the ASID MSR cost we care
about in a *fast* world (< 1 µs) but is invisible next to a 5 ms regression.
For precision we need a pure measurement.

### 3A. In-Kevlar micro-bench

Add `platform/arm64/asid_microbench.rs` with a kernel thread that:

- Runs once at boot, after SMP bring-up, from a dedicated kernel task.
- Loops 10 000 iterations:
  ```
  start = cntvct_el0;
  msr ttbr0_el1, <pgd | (iter << 48) | CnP>;
  isb;
  end = cntvct_el0;
  record(end - start);
  ```
- Reports mean/p50/p99 of the cycle counts, converts to ns via `cntfrq_el0`.
- Also runs a control loop with zero ASID bits for comparison.

Hypothesis from blog 216 predicts: the non-zero-ASID loop clocks ~5 ms per
write on HVF; the zero-ASID control is fast.  If so, we've reproduced the
pathology without any fork-path involvement.

### 3B. In-Linux equivalent

Patch Linux with the same loop (drop it into a module or a syscall), boot
and run.  If Linux's numbers are low (< 1 µs), we know our MSR is slow while
Linux's isn't — a reproduction we can bisect.  If Linux's numbers are *also*
high in the loop, the pathology is real on both and "Linux doesn't trigger
it in fork_exit" is the real mystery; reshape the investigation.

### 3C. Bare-metal repro

If 3A and 3B agree that this is a real HVF property (our numbers are high,
Linux's are also high in the same shape), strip to minimum: a 100-line
bare-metal arm64 binary that enables the MMU with AS=1, maps one user page
with nG, and runs the loop.  Publish as a QEMU-HVF reproducer.  (This is
also the artifact we'd file upstream if we conclude the bug is in HVF.)

## Phase 4: Deep instrumentation

If bit-bisection and micro-bench don't converge, we need to see *what HVF
is doing* during the MSR.

### 4A. QEMU logging

```
qemu-system-aarch64 ... -d int,mmu,cpu_reset -D /tmp/qemu.log
```

Grep for anything that fires per-switch.  HVF skips most emulation logs
because it doesn't run TCG, but ARM-spec register write/read logs may still
appear.

### 4B. `hvf_trace`

HVF emits trace events if built with `--enable-trace-backend=simple`.  If
our homebrew QEMU isn't built that way, build one that is.  Look for
`hvf_*` events around the MSR.

### 4C. Apple `powermetrics` / sample

`powermetrics --samplers cpu_power,tasks -n 1 -i 100` while the bench
loop runs.  Compare Linux vs Kevlar.  If one pinns a core at 100 % host CPU
in `com.apple.Virtualization` while the other doesn't, we know the host
handler is what costs.

### 4D. `dtrace` on `com.apple.Virtualization`

Apple's HVF stack surfaces dtrace probes (see `hv-trace` in the Hypervisor
framework docs).  Probe entry/exit points and count by guest-register cause
code.  Which host-side function does HVF call *only* on our kernel's TTBR
MSRs and not Linux's?

## Phase 5: Fallbacks

If Phases 1–4 can't explain the gap, accept the HVF limitation for now and
pivot the fork_exit arc to levers that don't depend on ASID tagging:

- **Process-struct pool** (noted in blog 213): reuse `Arc<Process>`
  allocations.  Estimated 1–2 µs/iter.
- **Reduce scheduler cost**: `process::switch()` is 3 µs/iter on arm64; a
  chunk of this is `do_switch_thread`'s register save/restore which was
  already trimmed in blog 215.  Inspect what's left.
- **Syscall-entry trim**: SVC → trap.S overhead.  Blog 215 removed FP/NEON
  save; the remaining path may still do unnecessary work.

## Ordering and estimated effort

| Phase | Effort | Expected info gain                                 |
|-------|--------|----------------------------------------------------|
| P1–P2 | 1 day  | Linux-on-HVF reproduction + baseline.              |
| 1     | 1 day  | Exhaustive register-bit diff.                      |
| 2     | 2 days | Bit bisection; likely where the cause is found.    |
| 3     | 1 day  | Isolated MSR cost, with or without fork.           |
| 4     | 2 days | Host-side HVF view.  Only needed if 1–3 fail.      |
| 5     | —      | Fallback — begin if 1–3 haven't converged.         |

Total: ~1 week of focused work to resolve or bound the gap.

## Open tactical questions

- **Which Linux version do we pin to?**  6.11 LTS is the obvious choice.
  Check for ASID-allocator / `cpu_do_switch_mm` refactors between 6.1 and
  6.11 — if there's a material change, pick the version that matches the
  stock macOS-guest or Asahi path.
- **QEMU version:** we're on 11.0.  HVF changed materially in 9.x → 11.x.
  Make sure we test both Linux and Kevlar on the same QEMU build.
- **Debug kernel vs production:** all measurements in RELEASE/balanced.
  Debug Kevlar will make the MSR noise floor way higher — don't mix.

## Definition of done

Either:

- A specific kernel-state change (one register bit, one extra write, one
  early-boot op) brings our fork_exit ASID cost into the < 10 µs/iter
  band, matching Linux; or
- A bare-metal reproducer that isolates the issue to HVF's emulation
  path, filed upstream with enough evidence to be actionable.

If neither is reached within the estimated budget, blog the findings,
commit the reproducer, and move to Phase 5 fallbacks.
