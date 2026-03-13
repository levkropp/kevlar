# M6.5 Phase 3: Scheduling Contracts

Phase 3 validates scheduling-related Linux contracts: nice values,
process priority, sched_yield, sched_getaffinity, and basic fork
scheduling fairness.

---

## Tests implemented

**nice_values** — Tests setpriority/getpriority round-trip for nice
values 0→5→10→19.  The test only increases nice (lower priority) since
Linux denies nice decrease for unprivileged users (EPERM).

**sched_yield** — Validates that `sched_yield()` returns 0 and
`sched_getaffinity()` returns at least 1 CPU.

**sched_fairness** — Forks a child, waits for it via `waitpid()`,
verifies the child ran and exited with the expected status.  This is
intentionally minimal: proper CFS weight testing is timing-sensitive
under QEMU TCG and prone to false failures.

**getpriority** — Already passing from Phase 1.5.

## Bug fix: sched_getaffinity return value

`sched_getaffinity` was returning 0 instead of the number of bytes
written.  musl uses this return value to determine how many bits to
scan in the cpu_set_t mask.  Returning 0 made `CPU_COUNT()` always
return 0.

```rust
// Before:
Ok(0)
// After:
Ok(size as isize)
```

## Known gaps

- **MAP_SHARED + fork**: Kevlar's fork deep-copies all pages, including
  MAP_SHARED mappings.  This breaks shared-memory IPC between parent
  and child.  A proper fix needs VMA flags tracking (`MAP_SHARED` vs
  `MAP_PRIVATE`) and page-table-level sharing during fork.  Tracked for
  future work.

- **Preemption latency test**: Skipped for now — requires `setitimer`
  and `SIGALRM` delivery (Phase 4 scope).

- **CFS weights**: No test for proportional CPU time distribution based
  on nice values.  The scheduler stores nice but doesn't use it for
  scheduling decisions yet.

## Results

| Test | Status |
|------|--------|
| scheduling.getpriority | PASS |
| scheduling.nice_values | PASS |
| scheduling.sched_fairness | PASS |
| scheduling.sched_yield | PASS |
| **Total** | **4/4 PASS** |

Full suite: **13/14 PASS** (sa_restart needs setitimer).
