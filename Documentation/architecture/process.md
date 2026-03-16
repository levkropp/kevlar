# Process & Thread Model

## Process Structure

A `Process` (`kernel/process/process.rs`) is the unit of resource ownership:

```rust
pub struct Process {
    pid: PId,
    tgid: PId,                  // Thread group leader PID
    state: AtomicCell<ProcessState>,
    parent: Weak<Process>,
    children: SpinLock<Vec<Arc<Process>>>,

    // Execution context
    arch: arch::Process,        // Saved registers, kernel stack, xsave FPU area

    // Shared resources (Arc for thread sharing)
    vm: AtomicRefCell<Option<Arc<SpinLock<Vm>>>>,
    opened_files: Arc<SpinLock<OpenedFileTable>>,
    signals: Arc<SpinLock<SignalDelivery>>,
    root_fs: AtomicRefCell<Arc<SpinLock<RootFs>>>,

    // Lock-free signal state
    signal_pending: AtomicU32,  // Mirror of signals.pending for fast-path check
    sigset: AtomicU64,          // Signal mask (lock-free Relaxed ordering)
    signaled_frame: AtomicCell<Option<PtRegs>>,

    // Identity
    uid: AtomicU32, euid: AtomicU32,
    gid: AtomicU32, egid: AtomicU32,
    umask: AtomicCell<u32>,
    nice: AtomicI32,
    comm: SpinLock<Option<Vec<u8>>>,
    cmdline: AtomicRefCell<Cmdline>,

    // Containers
    cgroup: AtomicRefCell<Option<Arc<CgroupNode>>>,
    namespaces: AtomicRefCell<Option<NamespaceSet>>,
    ns_pid: AtomicI32,          // Namespace-local PID

    // Thread support
    clear_child_tid: AtomicUsize,  // CLONE_CHILD_CLEARTID futex address
    vfork_parent: Option<PId>,

    // Accounting
    start_ticks: u64,
    utime: AtomicU64,
    stime: AtomicU64,

    // Diagnostics
    syscall_trace: SyscallTrace, // Lock-free ring buffer of last 32 syscalls
    // ...
}

pub enum ProcessState {
    Runnable,
    BlockedSignalable,
    Stopped(Signal),
    ExitedWith(c_int),
}
```

Atomic fields (`AtomicU32`, `AtomicU64`, `AtomicCell`) enable lock-free reads from
other CPUs — critical for signal delivery and scheduler decisions.

## Lifecycle

### fork

1. Check cgroup `pids.max` limit.
2. Allocate a new PID from the global process table.
3. Duplicate the page table with copy-on-write (writable pages get refcount bumped,
   WRITABLE bit cleared in both parent and child).
4. Copy the xsave FPU area from parent to child (preserves SSE/AVX state).
5. Clone the open file table, signal handlers, root filesystem, and CWD.
6. Inherit the parent's cgroup and namespace set; allocate a namespace-local PID.
7. Enqueue the child on the scheduler; child returns 0, parent returns child PID.

```rust
let vm = parent.vm().lock().fork()?;           // CoW page table copy
let opened_files = parent.opened_files().lock().clone();
let child = Arc::new(Process {
    pid, tgid: pid,  // New thread group leader
    vm: Some(Arc::new(SpinLock::new(vm))),
    opened_files: Arc::new(SpinLock::new(opened_files)),
    signals: Arc::new(SpinLock::new(SignalDelivery::new())),
    // ...
});
```

### vfork

Same as fork except:
- **No page table copy** — child shares the parent's address space.
- Parent is suspended until the child calls `execve` or `_exit`.
- Much faster than fork for the common fork+exec pattern.

### execve

1. Parse the ELF binary from the filesystem.
2. For PIE binaries: choose a base address and apply relocations.
3. For `PT_INTERP` (dynamic linking): load the interpreter (`ld-musl-*.so.1` or
   `ld-linux-*.so.2`) as a second ELF.
4. Kill all sibling threads (`de_thread` — POSIX requires execve to terminate all
   other threads in the thread group).
5. Reset signal handlers to `SIG_DFL` (handler addresses are no longer valid).
6. Rebuild the virtual memory map with ELF `PT_LOAD` segments.
7. Push `argv`, `envp`, and the auxiliary vector onto the new user stack.
8. Close `O_CLOEXEC` file descriptors.
9. Switch to the new page table and jump to the entry point.

Auxiliary vector entries: `AT_ENTRY`, `AT_BASE`, `AT_PHDR`, `AT_PHENT`, `AT_PHNUM`,
`AT_PAGESZ`, `AT_UID`, `AT_GID`, `AT_EUID`, `AT_EGID`, `AT_SECURE`, `AT_RANDOM`,
`AT_SYSINFO_EHDR`, `AT_HWCAP`, `AT_CLKTCK`.

### exit and wait

On `exit(2)`, the process:
1. Closes all open files and releases memory.
2. Reparents children to the subreaper or init (PID 1).
3. Clears the `clear_child_tid` address and wakes the futex (for `pthread_join`).
4. Marks itself as a zombie and sends `SIGCHLD` to its parent.
5. Wakes the parent's wait queue.

The parent collects the exit status via `wait4`. If the parent set
`sigaction(SIGCHLD, SIG_IGN)` (explicit ignore, not the default), children are
auto-reaped without becoming zombies (`nocldwait` flag).

`exit_group` kills all sibling threads (same `tgid`) before exiting.

### exit\_by\_signal

Signal-induced exits collect crash diagnostics:
- The last 32 syscalls from the per-process trace ring buffer
- The VMA map (up to 64 entries)
- Register state at the faulting instruction

These are emitted as structured JSONL debug events before the process terminates
with status `128 + signal`.

## Threads

Threads are created via `clone(CLONE_VM | CLONE_THREAD | CLONE_FILES | CLONE_SIGHAND)`.
A thread shares its parent's VM, file descriptor table, and signal handlers, but gets
its own PID (which serves as the TID), signal mask, and kernel stack:

```rust
pub fn new_thread(parent: &Arc<Process>, ...) -> Result<Arc<Process>> {
    let child = Arc::new(Process {
        pid,                                          // Unique TID
        tgid: parent.tgid,                            // Same thread group
        vm: parent.vm().clone(),                      // SHARED
        opened_files: Arc::clone(&parent.opened_files), // SHARED
        signals: Arc::clone(&parent.signals),         // SHARED handlers
        sigset: AtomicU64::new(parent.sigset_load().bits()), // Independent mask
        // ...
    });
    // ...
}
```

Thread exit clears `clear_child_tid` and performs a futex wake, enabling `pthread_join`
to detect thread completion.

## SMP Scheduler

The scheduler (`kernel/process/scheduler.rs`) implements per-CPU round-robin with
work stealing:

```rust
pub const MAX_CPUS: usize = 8;

pub struct Scheduler {
    run_queues: [SpinLock<VecDeque<PId>>; MAX_CPUS],
}
```

Each CPU has its own run queue. `pick_next` tries the local queue first for cache
warmth, then steals from other CPUs in round-robin order (stealing from the back
for fairness):

```rust
fn pick_next(&self) -> Option<PId> {
    let local = cpu_id() % MAX_CPUS;
    // Try local queue first
    if let Some(pid) = self.run_queues[local].lock().pop_front() {
        return Some(pid);
    }
    // Work stealing: try other CPUs
    for i in 1..MAX_CPUS {
        let victim = (local + i) % MAX_CPUS;
        if let Some(pid) = self.run_queues[victim].lock().pop_back() {
            return Some(pid);
        }
    }
    None
}
```

### Preemption

The LAPIC timer fires at 100 Hz. Every 3 ticks (30 ms), the current process is
preempted and rescheduled. The scheduler implements the `SchedulerPolicy` trait,
allowing the algorithm to be replaced without touching the platform crate.

### Per-CPU State

Each CPU maintains its own:
- `CURRENT`: the currently executing process (`Arc<Process>`)
- `IDLE_THREAD`: the idle thread (runs `hlt` when no work is available)
- Kernel stack cache for warm L1/L2 allocation during fork

## Job Control

Processes are organized into process groups and sessions:

- `setpgid` / `getpgid` — move a process into a process group
- `setsid` — create a new session (detach from controlling terminal)
- `tcsetpgrp` / `tcgetpgrp` — set/get the foreground group on a TTY

Background processes receive `SIGTTOU` on terminal write. Ctrl+Z sends `SIGTSTP` to
the foreground group. `SIGCONT` resumes stopped processes.

## cgroups v2

Each process belongs to a cgroup node. The hierarchy is managed via cgroupfs
(mounted at `/sys/fs/cgroup`):

```rust
pub struct CgroupNode {
    name: String,
    parent: Option<Weak<CgroupNode>>,
    children: SpinLock<BTreeMap<String, Arc<CgroupNode>>>,
    member_pids: SpinLock<Vec<PId>>,
    pids_max: AtomicI64,       // Enforced: fork returns EAGAIN if exceeded
    memory_max: AtomicI64,     // Stub
    cpu_max_quota: AtomicI64,  // Stub
    cpu_max_period: AtomicI64, // Stub
}
```

The **pids controller** is enforced: `fork`, `vfork`, and `clone` check the cgroup's
`pids.max` limit before allocating a PID. Memory and CPU controllers are stubs
(accepted but not enforced).

Children inherit their parent's cgroup membership on fork.

## Namespaces

Three namespace types are implemented:

### UTS Namespace

Per-namespace hostname and domainname. Default hostname: `"kevlar"`. Created via
`clone(CLONE_NEWUTS)` or `unshare(CLONE_NEWUTS)`.

### PID Namespace

Hierarchical PID isolation. Processes in a non-root PID namespace see namespace-local
PIDs starting at 1:

```rust
pub struct PidNamespace {
    parent: Option<Arc<PidNamespace>>,
    next_pid: AtomicI32,
    local_to_global: SpinLock<BTreeMap<PId, PId>>,
    global_to_local: SpinLock<BTreeMap<PId, PId>>,
}
```

`getpid()` returns `ns_pid` in non-root namespaces, the global PID otherwise.

### Mount Namespace

Per-namespace mount table. `pivot_root` is supported for container-style filesystem
isolation.

### NamespaceSet

```rust
pub struct NamespaceSet {
    pub uts: Arc<UtsNamespace>,
    pub pid_ns: Arc<PidNamespace>,
    pub mnt: Arc<MountNamespace>,
}
```

Namespaces are inherited on fork and can be selectively cloned with
`CLONE_NEWUTS`, `CLONE_NEWPID`, or `CLONE_NEWNS`.

## Capabilities

Linux capabilities are tracked as a bitmask. `prctl(PR_CAP_AMBIENT_*)` and
`capset`/`capget` manipulate the set. Operations requiring root (like `mount`)
check `CAP_SYS_ADMIN`. `prctl(PR_SET_CHILD_SUBREAPER)` designates the process
as the reaper for orphaned descendants.
