# Milestone 6.5: Linux Internal Contract Validation

**Goal:** Systematically verify that Kevlar implements Linux's internal contracts—the undocumented behavioral guarantees that existing software (especially GPU drivers) depends on—rather than just POSIX-level compatibility.

**Current state:** M6 demonstrates threading and basic syscalls work. But GPU drivers and complex userspace programs will expose subtle divergences in memory management, scheduling, signal delivery, and kernel subsystems.

**Impact:** M6.5 becomes the specification for M7-M10. By validating contracts systematically, we prevent discovering incompatibilities deep in GPU driver porting (M10). This is a unique approach: most compatibility layers guess; Kevlar validates.

## Strategic Rationale

POSIX is just the floor. Real Linux software depends on:
- **Memory contracts:** Page cache behavior, page flags, TLB shootdown timing, demand paging semantics
- **Scheduling contracts:** CFS (Completely Fair Scheduler) details, priority inheritance, deadline scheduling
- **Signal contracts:** Delivery ordering guarantees, signal mask semantics, coredump layout
- **Subsystem contracts:** DRM internals (drm_device, drm_mm), fbdev interfaces, /proc and /sys layout
- **Performance contracts:** Syscall latency expectations, lock ordering, context switch overhead

Linux's kernel internals are not documented. The only spec is the Linux source code itself. M6.5 builds comparative tests that capture this spec empirically.

## Phases

| Phase | Name | Key Focus | Prerequisite |
|-------|------|-----------|--------------|
| [1](phase1-test-harness.md) | Test Harness | Comparative runner (Linux vs Kevlar), result diffing | M6 |
| [2](phase2-vm-contracts.md) | VM Contracts | Demand paging, page cache, page flags, TLB semantics | Phase 1 |
| [3](phase3-scheduling-contracts.md) | Scheduling Contracts | CFS behavior, priority inheritance, deadline scheduling | Phase 1 |
| [4](phase4-signal-contracts.md) | Signal Contracts | Signal delivery ordering, mask semantics, coredump layout | Phase 1 |
| [5](phase5-subsystem-contracts.md) | Subsystem Contracts | DRM stubs, fbdev, /proc internals, /sys layout | Phase 1-4 |
| [6](phase6-program-compatibility.md) | Program Compatibility | Run progressively complex real programs, fix divergences | Phase 1-5 |

## Scope

### What M6.5 is:
- Comparative test suite (Linux vs Kevlar)
- Documentation of internal contracts
- Systematic bug-fixing pipeline
- Confidence signal for M7-M10

### What M6.5 is NOT:
- Rewriting Kevlar from scratch (keep what works)
- Implementing every Linux syscall (focus on contracts)
- Full desktop (that's M10)
- Performance optimization (correctness first)

## Methodology

1. **Test harness:** Tool that:
   - Boots Kevlar in QEMU + Linux in another QEMU/KVM instance
   - Runs identical test binaries on both
   - Compares output (stdout, exit code, files written)
   - Diffs divergences

2. **Contract tests:** For each subsystem, write tests that:
   - Probe behavior in isolation (e.g., "what happens when we write to a page, then fork?")
   - Document expected Linux behavior
   - Fail loudly if Kevlar diverges
   - Include comments explaining the contract

3. **Fix pipeline:** When a contract is violated:
   - Investigate root cause in Kevlar
   - Propose fix
   - Verify fix doesn't break M6 tests
   - Document the invariant

## Known Contract Gaps

Based on M6 investigation, likely divergences:

### VM Contracts
- Page eviction policy (Kevlar's demand-paging might not match Linux's LRU)
- Page flags (dirty, referenced, locked) — Kevlar might not track all
- TLB shootdown timing (IPI vs batching effects on memory visibility)

### Scheduling Contracts
- CFS weight calculations (Kevlar's per-CPU scheduler might diverge from Linux's global fair queue)
- Priority inheritance in futex (Kevlar stubs might not match Linux's PI implementation)
- Deadline scheduling (not implemented in Kevlar yet)

### Signal Contracts
- Signal delivery order (real-time vs non-real-time signal ordering)
- Coredump format (Kevlar might not generate glibc-compatible coredumps)
- Signal mask edge cases (masking SIGKILL, etc.)

### Subsystem Contracts
- DRM device numbers and permissions
- /proc layout and permission bits
- /sys hierarchy layout
- Device node behavior (ioctl dispatch, mmap semantics)

## Success Criteria

- [ ] Test harness boots both Linux and Kevlar and runs comparative tests
- [ ] VM contract tests pass on both (or divergences documented)
- [ ] Scheduling contract tests pass on both (or divergences documented)
- [ ] Signal contract tests pass on both (or divergences documented)
- [ ] Subsystem contract tests pass on both (or divergences documented)
- [ ] 10+ real programs (curl, vim, Python, gcc) run identically on both
- [ ] No M6 regressions when fixes are applied
- [ ] Contract documentation is discoverable for M7-M10 authors

## Future Work

After M6.5, the same comparative approach applies to M7-M10:
- **M7:** procfs/glibc contracts
- **M8:** cgroups/namespace contracts
- **M9:** systemd contracts
- **M10:** GPU driver contracts

Each milestone validates its own layer before the next builds on it.

## Reality Check

This is ambitious. M6.5 might take 2-3 weeks. But the payoff is high:
- GPU drivers in M10 become a "compile, test, fix bugs" task, not a "reverse-engineer undocumented internals" task
- M7-M9 can reference M6.5 contracts instead of guessing
- Kevlar becomes the first Linux-compatible kernel with explicit internal contract validation

If time is tight, phases can be prioritized: Phase 1 + 2 (VM) are highest-ROI for GPU drivers. Phases 3-5 are needed for systemd/containers (M8-M9).
