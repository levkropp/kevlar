# Phase 2: Namespaces — UTS, PID, and Mount Isolation

**Duration:** 4-5 days
**Prerequisite:** Phase 1 (cgroups v2)
**Goal:** Implement three namespace types: UTS (hostname), PID (process ID), and mount. Add `unshare(2)` and extend `clone(2)` with CLONE_NEWUTS, CLONE_NEWPID, CLONE_NEWNS. Stub CLONE_NEWNET with ENOSYS.

## Data Structures

**`kernel/namespace/mod.rs`** (new module)

```rust
pub struct NamespaceSet {
    pub uts: Arc<UtsNamespace>,
    pub pid_ns: Arc<PidNamespace>,
    pub mnt: Arc<MountNamespace>,
}
```

**`kernel/namespace/uts.rs`**

```rust
pub struct UtsNamespace {
    hostname: SpinLock<[u8; 65]>,
    hostname_len: AtomicUsize,
    domainname: SpinLock<[u8; 65]>,
    domainname_len: AtomicUsize,
}
```

**`kernel/namespace/pid_ns.rs`**

```rust
pub struct PidNamespace {
    parent: Option<Arc<PidNamespace>>,
    next_pid: AtomicI32,
    local_to_global: SpinLock<BTreeMap<PId, PId>>,
    global_to_local: SpinLock<BTreeMap<PId, PId>>,
}
```

**`kernel/namespace/mnt.rs`**

```rust
pub struct MountNamespace {
    root_fs: Arc<SpinLock<RootFs>>,
}
```

## Files to Create

1. **`kernel/namespace/mod.rs`** — NamespaceSet, clone flag constants, `init()`.
2. **`kernel/namespace/uts.rs`** — UtsNamespace with get/set hostname/domainname.
3. **`kernel/namespace/pid_ns.rs`** — PidNamespace with local/global PID translation.
4. **`kernel/namespace/mnt.rs`** — MountNamespace wrapping a cloned RootFs.
5. **`kernel/syscalls/unshare.rs`** — `sys_unshare(flags)` creates new namespaces in place.
6. **`kernel/syscalls/sethostname.rs`** — `sys_sethostname(name, len)` writes to UTS namespace.

## Files to Modify

1. **`kernel/process/process.rs`** — Add `namespaces: NamespaceSet` field. Initialize in all creation paths. Threads share parent's namespaces. Fork inherits; clone with CLONE_NEW* creates new.

2. **`kernel/syscalls/clone.rs`** — Handle CLONE_NEWUTS/NEWPID/NEWNS/NEWNET flags. Call `namespaces.clone_with_flags(flags)`. CLONE_NEWNET returns ENOSYS.

3. **`kernel/syscalls/mod.rs`** — Add `mod unshare; mod sethostname;`. Add syscall numbers (SYS_UNSHARE=272, SYS_SETHOSTNAME=170, SYS_SETDOMAINNAME=171 on x86_64). Add dispatch arms.

4. **`kernel/syscalls/uname.rs`** — Read hostname from `current_process().namespaces.uts` instead of hardcoded.

5. **`kernel/syscalls/getpid.rs`** — Return namespace-local PID when in a PID namespace.

6. **`kernel/fs/procfs/proc_self.rs`** — Add `"ns"` directory with uts/pid/mnt symlinks to ProcPidDir.

7. **`kernel/fs/mount.rs`** — `sys_mount` uses process's MountNamespace instead of global table.

## PID Namespace Design

- `alloc_pid()` always allocates a global PID (unchanged).
- When CLONE_NEWPID is set, child's PidNamespace assigns local PID 1.
- Process gains `ns_pid: PId` for namespace-local PID.
- `sys_getpid()` returns `ns_pid`, `sys_gettid()` returns global `pid`.
- When PID namespace init exits, all processes in namespace receive SIGKILL.

## Syscalls

| Syscall | x86_64 | ARM64 | Behavior |
|---------|--------|-------|----------|
| unshare | 272 | 97 | Create new namespace for calling process |
| sethostname | 170 | 161 | Set UTS hostname |
| setdomainname | 171 | 162 | Set UTS domainname |
| clone flags | existing | existing | CLONE_NEWUTS/NEWPID/NEWNS/NEWNET |

## Contract Tests

**`testing/contracts/subsystems/ns_uts.c`**
- Fork with CLONE_NEWUTS, child sets hostname, parent verifies isolation.

**`testing/contracts/subsystems/ns_pid.c`**
- Fork with CLONE_NEWPID, child sees PID 1, grandchild sees PID 2.

**`testing/contracts/subsystems/ns_unshare.c`**
- `unshare(CLONE_NEWUTS)`, set hostname, verify isolated.

## Success Criteria

- [ ] `unshare(CLONE_NEWUTS)` succeeds, hostname isolated
- [ ] `sethostname()` only affects calling UTS namespace
- [ ] `uname()` returns namespace-local hostname
- [ ] `clone(CLONE_NEWPID)` child sees PID 1
- [ ] `clone(CLONE_NEWNS)` child has independent mount table
- [ ] `clone(CLONE_NEWNET)` returns ENOSYS
- [ ] `/proc/[pid]/ns/` directory exists with uts, pid, mnt entries
- [ ] All existing contract tests pass
