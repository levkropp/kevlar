# Phase 1: cgroups v2 — Unified Hierarchy with Minimal Controllers

**Duration:** 3-4 days
**Prerequisite:** M7 complete
**Goal:** Implement the cgroups v2 unified hierarchy filesystem. `mount -t cgroup2 none /sys/fs/cgroup` produces a real cgroup tree. Three controllers (cpu, memory, pids) with pids.max enforced; cpu and memory are stubs.

## Data Structures

**`kernel/cgroups/mod.rs`** (new module)

```rust
pub struct CgroupNode {
    name: String,
    parent: Option<Weak<CgroupNode>>,
    children: SpinLock<BTreeMap<String, Arc<CgroupNode>>>,
    member_pids: SpinLock<Vec<PId>>,
    subtree_control: AtomicU32,  // bitflags: CPU=1, MEMORY=2, PIDS=4
    pids_max: AtomicI64,         // -1 = unlimited
    memory_max: AtomicI64,       // -1 = unlimited (stub)
    cpu_max: SpinLock<CpuMax>,   // stub
}
```

## Files to Create

1. **`kernel/cgroups/mod.rs`** — CgroupNode struct, controller bitflags, `CGROUP_ROOT: Once<Arc<CgroupNode>>`, `init()`.

2. **`kernel/cgroups/cgroupfs.rs`** — CgroupFs (FileSystem), CgroupDir (Directory), CgroupControlFile (FileLike read/write).

3. **`kernel/cgroups/pids_controller.rs`** — `check_fork_allowed(cgroup)` walks up the tree counting PIDs against limits. Returns `Err(EAGAIN)` if over.

## Files to Modify

1. **`kernel/syscalls/mount.rs`** — Replace `"cgroup2" | "cgroup"` tmpfs stub with `CgroupFs::new_or_get()` singleton.

2. **`kernel/process/process.rs`** — Add `cgroup: Arc<CgroupNode>` field. Initialize to root cgroup in `new_idle_thread()`, `new_init_process()`, `fork()`, `new_thread()`.

3. **`kernel/process/process.rs` (fork)** — Call `pids_controller::check_fork_allowed(&parent.cgroup)?` before `alloc_pid()`.

4. **`kernel/fs/procfs/proc_self.rs`** — Add `"cgroup"` to ProcPidDir::lookup() returning `"0::/<cgroup_path>\n"`. Add to readdir entries.

5. **`kernel/main.rs`** — Call `cgroups::init()` during boot.

6. **`testing/Dockerfile`** — Add COPY line for `cgroup_basic.c`.

## CgroupFs File Interface

CgroupDir::lookup() returns these files per cgroup directory:

| File | Read | Write |
|------|------|-------|
| `cgroup.procs` | Lists member PIDs | Move PID to this cgroup |
| `cgroup.controllers` | Available controllers from parent | — |
| `cgroup.subtree_control` | Enabled controllers | `+pids -cpu` format |
| `cgroup.type` | `"domain\n"` | — |
| `cgroup.stat` | nr_descendants, nr_dying_descendants | — |
| `pids.max` | Current limit | Set limit (integer or "max") |
| `pids.current` | Count of PIDs in subtree | — |
| `memory.max` | `"max\n"` (stub) | Parse and store (stub) |
| `memory.current` | `"0\n"` (stub) | — |
| `cpu.max` | `"max 100000\n"` (stub) | Parse and store (stub) |

CgroupDir::create_dir() creates a child CgroupNode. CgroupDir::rmdir() removes empty child cgroups.

## cgroup.procs Write Semantics

1. Parse PID from user buffer
2. Find process via `Process::find_by_pid()`
3. Remove PID from old cgroup's `member_pids`
4. Add PID to new cgroup's `member_pids`
5. Update process's `cgroup` field

## pids.max Enforcement

In `Process::fork()`, before allocating a PID:
1. Walk from parent's cgroup up to root
2. At each level where `pids_max != -1`, count all PIDs in that subtree recursively
3. If count >= max at any level, return `Err(EAGAIN)`

## Contract Test

**`testing/contracts/subsystems/cgroup_basic.c`**
1. `mount("cgroup2", "/sys/fs/cgroup", "cgroup2", 0, NULL)`
2. Read `cgroup.controllers` — verify readable
3. `mkdir("/sys/fs/cgroup/test.scope", 0755)` — create child cgroup
4. Read `/proc/self/cgroup` — verify format `"0::/<path>\n"`
5. Write own PID to `test.scope/cgroup.procs`
6. Re-read `/proc/self/cgroup` — verify path changed to `/test.scope`
7. Write `"+pids"` to `cgroup.subtree_control`
8. Create sub-cgroup, set `pids.max = 2`, verify fork rejection

## Success Criteria

- [ ] `mount -t cgroup2` succeeds and shows control files
- [ ] `mkdir` creates child cgroups with inherited controllers
- [ ] Writing PID to `cgroup.procs` moves process
- [ ] `/proc/self/cgroup` returns correct path
- [ ] `pids.max` enforcement: fork returns EAGAIN at limit
- [ ] `memory.max` and `cpu.max` files readable/writable (stubs)
- [ ] All existing contract tests still pass
