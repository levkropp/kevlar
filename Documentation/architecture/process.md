# Process & Thread Model

## Process Structure

A `Process` (`kernel/process/process.rs`) is the unit of resource ownership. It holds:

| Field | Type | Purpose |
|---|---|---|
| `pid` | `i32` | Process ID |
| `ppid` | `i32` | Parent PID |
| `pgid` | `i32` | Process group ID |
| `sid` | `i32` | Session ID |
| `task` | `Task` | Platform execution context (registers, stack, FPU) |
| `vm` | `Arc<SpinLock<Vm>>` | Virtual memory map (VMAs + page table) |
| `opened_files` | `Arc<SpinLock<OpenedFileTable>>` | File descriptor table |
| `signal_delivery` | `SignalDelivery` | Signal handlers, mask, pending set |
| `signal_pending` | `AtomicU32` | Fast-path pending signal bitmask |
| `cwd` | `PathComponent` | Current working directory |
| `root` | `PathComponent` | Root directory (for chroot) |

`Arc<SpinLock<...>>` on `vm` and `opened_files` supports `clone(2)` with shared
resources (as used by `pthread_create` with `CLONE_VM | CLONE_FILES`).

## Lifecycle

### fork

1. Duplicate the parent's page table (copy-on-write is not yet implemented; pages
   are copied eagerly).
2. Copy the parent's xsave FPU area to the child task.
3. Clone the open file table (with `dup`'d references) and VMA list.
4. The child returns 0 from `fork`; the parent returns the child's PID.

### execve

1. Parse the ELF binary from the filesystem.
2. For PIE binaries: choose a random or fixed base address; apply relocations.
3. For `PT_INTERP` (dynamic linking): load the interpreter (`ld-musl-*.so.1`) as
   a second ELF, set entry point to interpreter entry.
4. Rebuild the process's virtual memory map with ELF segments.
5. Push `argv`, `envp`, and the auxiliary vector (auxv) onto the new user stack.
6. Reset all signal handlers to SIG_DFL (POSIX requirement for `execve`).
7. Jump to the entry point.

Auxiliary vector entries provided: `AT_ENTRY`, `AT_BASE`, `AT_PHDR`, `AT_PHENT`,
`AT_PHNUM`, `AT_PAGESZ`, `AT_UID`, `AT_GID`, `AT_EUID`, `AT_EGID`,
`AT_SECURE`, `AT_RANDOM`, `AT_SYSINFO_EHDR`.

### exit and wait

On `exit(2)`, the process releases its open files and memory, marks itself as a
zombie, and wakes its parent's wait queue. The parent collects the exit status via
`wait4`.

If the parent has called `sigaction(SIGCHLD, SIG_IGN)` (explicit ignore, not the
default), children are auto-reaped on exit without becoming zombies
(`nocldwait` flag in `SignalDelivery`).

## Scheduler

The scheduler (`kernel/process/scheduler.rs`) implements cooperative + preemptive
round-robin scheduling:

- **Run queue:** A FIFO list of runnable processes.
- **Preemption:** The APIC timer fires at `TICK_HZ = 100` Hz and preempts the current
  process every `PREEMPT_PER_TICKS = 3` ticks (30 ms time slice).
- **Sleeping:** Processes block on `WaitQueue::wait_event` and are woken by
  `WaitQueue::wake_all` / `wake_one`.

The scheduler implements the `SchedulerPolicy` trait (Ring 2 boundary), allowing
the policy to be replaced without touching the platform crate.

## Job Control

Processes are organized into process groups and sessions for terminal job control:

- `setpgid` / `getpgid` — move a process into a process group
- `setsid` — create a new session (detach from controlling terminal)
- `tcsetpgrp` / `tcgetpgrp` — set/get the foreground process group on a TTY

When a background process writes to a terminal, it receives `SIGTTOU`. Pressing
Ctrl+Z sends `SIGTSTP` to the foreground process group. `SIGCONT` resumes it.

## Capabilities

Linux capabilities are tracked as a bitmask in `ProcessData`. `prctl(PR_CAP_AMBIENT_*)`
and `capset`/`capget` manipulate the set. Most capability checks are advisory;
operations that require root (like `mount`) check `CAP_SYS_ADMIN`.
