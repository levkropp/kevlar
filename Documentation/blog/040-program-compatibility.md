# M6.5 Phase 6: Program Compatibility

Phase 6 validates that Kevlar can run real programs by exercising
multiple kernel contracts simultaneously.

---

## Tier 1: fork + exec + wait

The `busybox_basic` test validates the core process lifecycle: fork a
child, check exit status via waitpid, verify parent PID is correct.
This exercises fork(), execve() (indirectly through _exit), waitpid(),
getpid(), and getppid() — the foundation that BusyBox shell and all
higher-tier programs depend on.

Tests: fork with exit codes 0/1/42, 5 sequential children, getppid
across fork boundary.

## Known gaps for future tiers

- **Tier 2 (dynamic musl)**: hello-dynamic works, but the contract
  test framework doesn't yet test it (needs dynamic binary execution
  via execve, not just static compilation).

- **Tier 3 (glibc)**: Needs FUTEX_CMP_REQUEUE, rseq stub, clone3 stub.
  These are M7 scope.

- **Tier 4 (system utilities)**: Needs `/proc/[pid]/maps`,
  `/proc/cpuinfo` format validation.  M7 scope.

- **Tier 5-7**: Python, networking, GPU — M8-M10 scope.

## M6.5 Milestone Summary

| Phase | Tests | Pass | Known Gaps |
|-------|-------|------|------------|
| 1 | Test harness | N/A | — |
| 1.5 | Trace tooling | N/A | — |
| 2 | VM (8 tests) | 8/8 | MAP_SHARED+fork |
| 3 | Scheduling (4) | 4/4 | CFS weights, preemption |
| 4 | Signals (4) | 3/4 | sa_restart (needs alarm) |
| 5 | Subsystems (2) | 2/2 | /proc/cpuinfo, /sys |
| 6 | Programs (1) | 1/1 | Tiers 2-7 |
| **Total** | **19** | **18/19** | — |

The single remaining failure (`sa_restart`) requires `setitimer`/`alarm`
delivery, tracked for M7.

## Kernel fixes shipped in M6.5

| Fix | Impact |
|-----|--------|
| brk() never returns error | musl sbrk compatibility |
| PROT_NONE delivers SIGSEGV to handler | Signal handler + longjmp works |
| getpriority/setpriority | Process priority management |
| sched_getaffinity returns byte count | CPU_COUNT() works correctly |
| /dev/zero | Zero-fill device node |
| Runtime debug=syscall cmdline | Zero-recompile tracing |
| Dockerfile COPY fix | /etc files in initramfs |
