# Future Claude Code Session Prompt — ARM64 validation + graphical-desktop continuation

**Copy everything below into a new Claude Code session running on an ARM64 MacBook.**

---

You are resuming the Kevlar kernel project — a permissively-licensed
Rust drop-in replacement for the Linux kernel.  The previous session
(see `Documentation/blog/206-session-summary-task-25-investigation.md`)
landed a partial fix for task #25 (kernel-pointer-in-user-page leak)
and left the graphical-desktop work in a partially-validated state
on x86_64.

Your environment: ARM64 (aarch64) MacBook with QEMU installed.  Your
job has two phases: (1) validate Kevlar still works on ARM64 and
produce fresh benchmarks vs Linux ARM64 KVM and the contract suite,
then (2) resume the graphical-desktop work.

## Phase 1 — ARM64 validation (do first, independently)

### 1A. Build and smoke-test

```bash
cd ~/kevlar
rustup override set nightly
rustup target add aarch64-unknown-none
make ARCH=arm64 RELEASE=1 check     # type-check only, fast
make ARCH=arm64 RELEASE=1 build     # full build (debug is too slow for arm64 TCG)
```

If `check` fails: report the error.  Do not proceed without a clean
type-check.  Some arm64 builds may hit stale symbol issues if the
workspace was last built for x86_64; `cargo clean -p kevlar_platform`
is often enough.

### 1B. Boot + BusyBox sanity

```bash
make ARCH=arm64 RELEASE=1 run
```

Expected: BusyBox shell prompt reaches `/ #` within ~20 s and
`uname -a` shows Kevlar.  If it hangs before shell, capture the
last 100 lines of serial output and investigate.  The ARM64 boot
path shares most of its logic with x86_64 but has its own
`platform/arm64/` module — common hang sites historically are the
PSCI trampoline and virt-scan MADT equivalents.

### 1C. Contract tests

```bash
make ARCH=arm64 RELEASE=1 test-contracts-arm64
```

Expected: ≥95/118 tests pass (baseline established 2026-03-19, see
`project_arm64_contract_tests.md` memory).  Any regression from that
baseline is a bug — investigate before proceeding.  Expected-pass
test classes: process lifecycle, fd table, mmap, signals, pipes.
Expected-fail (known): some clock and cgroup tests (M7/M8 work).

### 1D. Threading regression

```bash
make ARCH=arm64 RELEASE=1 test-threads-smp
```

Expected: 14/14 on `-smp 2` under TCG (arm64 KVM on Mac requires
Hypervisor.framework — covered in 1E).  If fewer than 14 pass, check
which thread test failed — the usual suspects on ARM64 are memory
ordering (DSB barriers) and TLB invalidation scope.

### 1E. ARM64 KVM via Hypervisor.framework

```bash
make ARCH=arm64 RELEASE=1 build
qemu-system-aarch64 -machine virt -accel hvf -cpu host \
    -m 1024 -smp 2 -serial stdio -nographic \
    -kernel kevlar.arm64.elf
```

`-accel hvf` uses macOS's Hypervisor.framework — the arm64
equivalent of KVM.  If this boots cleanly, we have fast-path ARM64
execution for benchmarks.  If it panics with "unsupported
instruction," drop to TCG and note the class of failure.

### 1F. Benchmark vs Linux ARM64 KVM

On the same MacBook, build a Linux ARM64 kernel (or use the Alpine
ARM64 qcow2) and run the bench suite (`benchmarks/bench.c`) on both:

```bash
# Kevlar
make ARCH=arm64 RELEASE=1 bench-arm64 2>&1 | tee /tmp/kevlar-arm64-bench.log

# Linux comparison (assuming build/alpine-arm64.img exists)
make bench-linux-arm64 KERNEL=/path/to/linux-arm64-kernel \
    DISK=build/alpine-arm64.img 2>&1 | tee /tmp/linux-arm64-bench.log

# Compare
python3 benchmarks/run-benchmarks.py compare \
    /tmp/kevlar-arm64-bench.log /tmp/linux-arm64-bench.log
```

Expected shape: Kevlar within 10% of Linux for syscall-heavy
benchmarks (getpid, read, write), within 20% for mmap-heavy ones
(mmap_fault).  The x86_64 M6.6 target was "27/28 within 10%" — any
aarch64-specific gap beyond 20% is a performance regression worth
blogging about.

**Before proceeding to Phase 2**, write a short status report:

- Contract tests: X/118 passing (change from 2026-03-19 baseline: Δ)
- Threading SMP: Y/14
- ARM64 KVM (hvf): boots / doesn't
- Benchmarks vs Linux: summary line per test, worst-case ratio

Commit this as `blog 207 ARM64 re-validation YYYY-MM-DD` so the next
session has a fresh baseline.

## Phase 2 — Resume graphical-desktop work

After ARM64 is confirmed healthy.  The main kernel bug on the
desktop stack is task #25 (kernel-pointer leak into user pages).
It's been reduced from ~40% → ~16% of XFCE runs via the defensive
`cld` fixes (see blog 205).  The remaining 16% is a different
mechanism — the page is clean at alloc time, gets corrupted during
user use.

### 2A. Read the instrumentation baseline

```bash
cat Documentation/blog/206-session-summary-task-25-investigation.md
cat ~/.claude/projects/-home-fw-kevlar/memory/project_leak_instrumentation.md
```

You'll see 11 always-on detectors + `tools/analyze-leak-log.py`.  All
of them run every XFCE test.  Start here, don't reinvent.

### 2B. Reproduce the residual leak

```bash
for i in $(seq 1 20); do
    make test-xfce PROFILE=balanced 2>&1 | tail -1
    cp /tmp/kevlar-test-xfce-balanced.log /tmp/kevlar-xfce-$i.log
done
python3 tools/analyze-leak-log.py /tmp/kevlar-xfce-*.log
```

Expected: ~3/20 runs with KERNEL_PTR_LEAK events, all with the
signature `paddr=0x2d2d760` in xfwm4.  If the rate is materially
higher or different, something's regressed since this session — read
the current blog posts starting from 206 backward to understand
context.

### 2C. Task #19 — narrow the residual mechanism

The residual leaks fire WITHOUT accompanying PAGE_ZERO_MISS events,
which means the page is clean when handed to the user and gets
corrupted AFTER.  Candidates:

1. **Stale user TLB on a remote CPU writes via the stale translation
   after munmap → free → realloc.**  My per-CPU PCID fix and CoW
   free-before-flush closed the obvious paths; check if another
   path bypasses the broadcast IPI.
2. **The huge-page cache** (`HUGE_PAGE_CACHE` in page_fault.rs:213)
   stores raw PAddrs but wasn't audited for refcount-pinning the
   way the regular PAGE_CACHE was — see if huge pages freed under
   us are still referenced by the cache.
3. **Fault-around / prefetch paths** pre-allocate pages.  If a
   prefetched paddr gets freed by a different path before the user
   accesses it, user writes through stale translation to a paddr
   the allocator has handed to someone else.
4. **An inline-rep site I missed** — grep the codebase again for
   `rep stos\|rep movs` and ensure each has `cld`.  Test by
   disabling the switch-boundary cld (revert commit `7ae6f15`,
   rebuild, re-run 20 tests).  If rate stays at ~16%, the switch-
   boundary cld wasn't helping and all the rep sites ARE covered
   per-primitive.  If rate jumps to 40%, switch-boundary cld IS
   catching a gap.

Report findings in a new blog post.

### 2D. Then close task #6 / #10 / #11 stack (graphical)

After task #19 closes, XFCE should be >95% reliable.  At that point:

- Re-run 20 `test-xfce` runs — aim for 0 KERNEL_PTR_LEAK, ≥18/20
  reach TEST_END with score 4/5 or 5/5
- Run 20 `test-lxde` runs — aim for ≥18/20 reach 6/6
- Document the state in blog 208 / 209 and update MEMORY.md

Only then should we consider the next graphical DE (GNOME is out of
scope; try Fluxbox or IceWM as even-lighter comparisons to LXDE).

### 2E. Known-good reference

If at any point you can't tell whether behavior is a regression or
expected, here are the reference states:

- `a6cbbe8` (blog 203): PCID per-CPU gen — baseline for all subsequent
  TLB work
- `bc02033` (blog 205 finalized): cld fix — latest stable main
- XFCE test-xfce: ~16% KERNEL_PTR_LEAK rate, 5/5 score when clean
- test-threads-smp: 14/14 on both x86_64 and arm64

## Non-goals for this session

- M7 /proc filesystem work (separate milestone)
- M8 cgroups (separate milestone)
- Removing instrumentation from tree — keep it all, it's load-bearing
  for future investigations
- Attempting to fix the residual 16% without first reproducing + analyzing
  with existing instrumentation

## If you get stuck

1. Read the most recent 5 blog posts in `Documentation/blog/` (by
   number).  They're chronological and each explains the state at
   the time.
2. Read `~/.claude/projects/-home-fw-kevlar/memory/MEMORY.md` and the
   referenced topic files — they capture findings that don't fit in
   blog format.
3. Run `git log --oneline -30` — recent commit messages are the
   change log.
4. If a test hangs in QEMU, `pkill -9 qemu-system-aarch64` to kill
   it.  The test-xfce harness has a 300 s timeout but `run-qemu.py`
   can wedge on QMP socket issues.

## Success criteria for this session

- ARM64 validation status documented (blog 207)
- Task #19 closed OR reduced to a single specific hypothesis with a
  reproducer
- XFCE or LXDE reaches ≥90% reliability (KERNEL_PTR_LEAK rate ≤5%)
- At least one new blog post documenting progress

Good luck.  The instrumentation does most of the heavy lifting now —
pay attention to what it tells you before inventing new hypotheses.
