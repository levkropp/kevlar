# Milestone 8: cgroups v2 & Namespaces

**Goal:** Implement the container isolation primitives that systemd requires.
Enable process isolation via namespaces and resource limits via cgroups.

**Current state:** Kevlar has basic process management and /proc filesystem
(M7). systemd cannot boot without cgroups v2 (for service management) and
namespaces (for container-like service isolation).

**Impact:** This is the last major kernel feature needed before systemd can
start. Once M8 is done, M9 (running systemd) becomes feasible.

## Phases

| Phase | Name | Key Changes | Prerequisite |
|-------|------|------------|--------------|
| [1](phase1-cgroups-v2.md) | cgroups v2 | Unified hierarchy, minimal controllers (cpu, memory, pids) | M7 |
| [2](phase2-namespaces.md) | Namespaces | CLONE_NEWNS, CLONE_NEWPID, CLONE_NEWUTS, CLONE_NEWNET | M7 + Phase 1 |
| [3](phase3-pivot-root.md) | Filesystem Isolation | pivot_root, mount namespace implementation | Phase 2 |
| [4](phase4-integration.md) | Systemd Boot | systemd-init binary runs and manages basic services | Phase 1-3 |

## Architectural Impact

### cgroups v2 (Large Impact)

cgroups are a major subsystem. v2 (unified hierarchy) is simpler than v1.

Key concepts:
- **Hierarchy:** Single tree rooted at `/sys/fs/cgroup/`. Each process belongs
  to exactly one cgroup at each level.
- **Controllers:** cpu, memory, pids, io, etc. Only enabled controllers can be
  written to. Start with cpu, memory, pids.
- **Interface:** Writing to cgroup.subtree_control enables children. Writing to
  cgroup.procs moves the process.
- **Resource limits:** memory.max, cpu.max (weight + period), pids.max
- **Delegation:** systemd creates child cgroups per service, sets limits,
  moves processes.

Implementation:
- Extend Process struct with `cgroup_path: String` (e.g., "/user.slice/session.scope")
- New kernel subsystem: `kernel/cgroups/` with controllers
- sysfs integration: expose cgroups under `/sys/fs/cgroup/`
- Syscalls: `cgroup_migrate` (move process to cgroup) — might use cgroup.procs write

### Namespaces (Very Large Impact)

Namespaces are also major. Each process can have different views of:
- **Mount (CLONE_NEWNS):** Filesystem mount points
- **PID (CLONE_NEWPID):** Process tree (PID 1 in the namespace is different from global PID)
- **UTS (CLONE_NEWUTS):** Hostname, domainname
- **Network (CLONE_NEWNET):** Network interfaces, routes, sockets
- **User (CLONE_NEWUSER):** UID/GID mappings (complex, skip initially)
- **IPC (CLONE_NEWIPC):** Shared memory, semaphores (skip initially)

Start with NEWNS (mount), NEWPID (process view), NEWUTS (hostname).

Implementation:
- Extend Process struct with `namespaces: NamespaceSet` containing pointers
  to mount table, PID namespace root, UTS struct
- clone() with CLONE_NEWNS creates new namespace
- Subsequent path lookups use the process's namespace
- `/proc/[pid]/ns/` contains symlinks to namespace descriptors

### Filesystem Isolation (Medium Impact)

Supporting mount namespaces requires:
- Per-process mount table
- `pivot_root()` syscall for changing filesystem root
- Mount propagation (private, shared, slave) — complex, can defer

## Test Plan

**Acceptance criteria:**
- systemd can boot and reach a state where it tries to manage services
- Processes can be placed in cgroups and limits enforced
- Each cgroup sees different hostname (UTS namespace)
- Mount namespace allows private /tmp per service

**Test scenario:**
```bash
mkdir -p /sys/fs/cgroup
mount -t cgroup2 none /sys/fs/cgroup
systemd --version
systemctl status
```

Should output systemd version and list of services (even if most fail to start).

## Known Challenges

1. **Namespace interactions:** PID namespace affects process lookup globally.
   Changing PID namespace root requires careful handling of the scheduler and
   process table.

2. **Mount namespace complexity:** Each mount operation affects only the
   current namespace. Propagating mounts across namespaces is non-trivial.

3. **cgroups accounting:** Resource usage must be accurately tracked across
   processes. Requires CPU cycle counters and memory accounting in the
   allocator.

4. **Delegation:** systemd delegates cgroup control to services (running as
   unprivileged users). Requires careful permission checks.

## Success Metrics

- [ ] cgroups v2 hierarchy is visible under /sys/fs/cgroup
- [ ] cpu.max, memory.max, pids.max can be set and are enforced
- [ ] CLONE_NEWNS works; mount points are namespace-local
- [ ] CLONE_NEWPID works; processes see different PID 1
- [ ] CLONE_NEWUTS works; different namespaces see different hostname
- [ ] systemd can start and recognize cgroups v2
- [ ] systemd-run can launch services in isolated cgroups
