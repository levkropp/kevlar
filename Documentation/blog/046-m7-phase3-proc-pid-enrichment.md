# M7 Phase 3: Per-process /proc Enrichment

Phase 3 adds CPU time accounting, process start time, and thread
counting to the Process struct, then wires real values into
/proc/[pid]/stat and /proc/[pid]/status.

## The problem

The existing /proc/[pid]/stat emitted 52 fields but almost all were
hardcoded zeros.  The state was always 'S', utime/stime were always 0,
num_threads was always 1, and vsize/rss were always 0.  Similarly,
/proc/[pid]/status hardcoded `State: S (sleeping)`, `Uid: 0 0 0 0`,
and `Threads: 1` regardless of the actual process.  Tools like `ps`,
`top`, and `htop` rely on these fields being accurate.

## CPU time accounting

Three new fields on the Process struct:

```rust
start_ticks: u64,       // monotonic ticks at creation
utime: AtomicU64,       // user-mode ticks
stime: AtomicU64,       // kernel-mode ticks
```

User time is approximated by incrementing `utime` in the timer IRQ
handler for whichever non-idle process was running when the tick fired.
Kernel time is approximated by incrementing `stime` once per syscall
entry.  Neither is high-precision, but both are the standard approach
for tick-based kernels and match what Linux does with its statistical
sampling.

The fields are initialized in all four process creation paths:
`new_idle_thread`, `new_init_process`, `fork`, and `new_thread`.  Each
captures `monotonic_ticks()` as the start time.

## Thread counting and VM size

Two new methods on Process:

- `count_threads()` — locks PROCESSES and counts entries sharing the
  same TGID.  This replaces the hardcoded `1` in /proc/[pid]/status
  and the zero in /proc/[pid]/stat field 20.

- `vm_size_bytes()` — sums VMA lengths from the process's Vm.  This
  was previously computed inline in the status file handler; extracting
  it to Process lets both stat and status share the same logic.

## /proc/[pid]/stat fields

The stat file now reports real values for:

| Field | Name        | Source                        |
|-------|-------------|-------------------------------|
| 3     | state       | ProcessState -> R/S/T/Z       |
| 14    | utime       | process.utime() atomic        |
| 15    | stime       | process.stime() atomic        |
| 20    | num_threads | count_threads()               |
| 22    | starttime   | process.start_ticks()         |
| 23    | vsize       | vm_size_bytes()               |
| 24    | rss         | vsize / PAGE_SIZE (approx)    |

## /proc/[pid]/status fields

The status file now reports:

- **State** — mapped from ProcessState (`R (running)`, `S (sleeping)`,
  `T (stopped)`, `Z (zombie)`)
- **Uid/Gid** — read from the process's uid/euid/gid/egid atomics
  instead of hardcoded zeros
- **VmSize/VmRSS** — from `vm_size_bytes()` (shared implementation)
- **Threads** — from `count_threads()` instead of hardcoded 1

## Contract test

The new `proc_pid.c` test verifies:

- /proc/self/stat field 1 (pid) matches `getpid()`
- /proc/self/stat field 3 (state) is 'R' while actively running
- /proc/self/stat field 20 (num_threads) is >= 1
- /proc/self/status contains `Name:` and `Pid:` matching `getpid()`

## Results

22/22 contract tests pass (5/5 subsystem tests including the new
proc_pid).

## What's next

Phase 4 adds /proc/[pid]/mountinfo and /proc/[pid]/cgroup — two files
that glibc and systemd read during early init to discover the mount
namespace and cgroup membership.
