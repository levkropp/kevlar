# Phase 4: Integration Testing

**Duration:** 2-3 days
**Prerequisite:** Phases 1-3
**Goal:** Integration test binary exercising all M8 features together. Verify backwards compatibility. Prepare for M9 (systemd) by mimicking systemd's cgroup+namespace boot sequence.

## Integration Test: mini_cgroups_ns.c

14 subtests running as PID 1:

```
 1. cgroup_mount       — mount -t cgroup2, verify root files
 2. cgroup_mkdir       — create child cgroup directory
 3. cgroup_move_procs  — write PID to cgroup.procs, verify /proc/self/cgroup
 4. cgroup_subtree_ctl — enable +pids in subtree_control
 5. cgroup_pids_max    — set pids.max=2, fork until EAGAIN
 6. ns_uts_isolate     — clone(CLONE_NEWUTS), sethostname, verify isolation
 7. ns_uts_unshare     — unshare(CLONE_NEWUTS), verify sethostname local
 8. ns_pid_basic       — clone(CLONE_NEWPID), child sees PID 1
 9. ns_pid_nested      — child forks in PID ns, grandchild sees PID 2
10. ns_mnt_isolate     — clone(CLONE_NEWNS), mount is private
11. proc_cgroup        — /proc/self/cgroup returns correct path
12. proc_mountinfo     — /proc/self/mountinfo has correct format
13. proc_ns_dir        — /proc/self/ns/ contains uts, pid, mnt
14. systemd_boot_seq   — full systemd-like cgroup setup sequence
```

## systemd_boot_seq Subtest

Mimics systemd's actual early boot:
1. `mount("cgroup2", "/sys/fs/cgroup", "cgroup2", 0, NULL)`
2. Read `cgroup.controllers` — expect `"cpu memory pids"`
3. Write `"+cpu +memory +pids"` to `cgroup.subtree_control`
4. `mkdir("/sys/fs/cgroup/init.scope", 0755)`
5. Move PID 1 into `init.scope/cgroup.procs`
6. `mkdir("/sys/fs/cgroup/system.slice", 0755)`
7. Verify `/proc/self/cgroup` shows `/init.scope`
8. Set `pids.max = 100` on `system.slice`

## New Contract Tests (7)

| Test | Category | Exercises |
|------|----------|-----------|
| `cgroup_basic` | subsystems | cgroup2 mount, hierarchy, procs |
| `cgroup_pids` | subsystems | pids.max enforcement |
| `ns_uts` | subsystems | UTS namespace isolation |
| `ns_pid` | subsystems | PID namespace isolation |
| `ns_unshare` | subsystems | unshare syscall |
| `ns_mount` | subsystems | mount namespace isolation |
| `mountinfo` | subsystems | /proc/[pid]/mountinfo format |

## Files to Create

1. **`testing/mini_cgroups_ns.c`** — 14-subtest integration binary.
2. **`testing/contracts/subsystems/cgroup_pids.c`** — pids.max contract.
3. **`testing/contracts/subsystems/ns_mount.c`** — mount namespace contract.

## Files to Modify

1. **`Makefile`** — Add `test-mini-cgroups-ns` and `test-m8` targets.
2. **`testing/Dockerfile`** — Add COPY lines and build stage for mini_cgroups_ns.

## Makefile Targets

```makefile
test-m8:
    $(MAKE) test-glibc-hello
    $(MAKE) test-glibc-threads
    $(MAKE) test-threads-smp
    $(MAKE) test-regression-smp
    $(MAKE) test-contracts
    $(MAKE) test-mini-cgroups-ns
```

## Success Criteria

- [ ] `mini_cgroups_ns` passes 14/14 subtests
- [ ] All 7 new contract tests pass (Linux = Kevlar)
- [ ] All existing contract tests pass (26+)
- [ ] 14/14 musl thread tests pass on -smp 4
- [ ] 14/14 glibc thread tests pass on -smp 4
- [ ] `systemd_boot_seq` subtest passes
- [ ] `make test-m8` runs complete suite

## Duration Summary

| Phase | Duration | Cumulative |
|-------|----------|------------|
| 1: cgroups v2 | 3-4 days | 3-4 days |
| 2: Namespaces | 4-5 days | 7-9 days |
| 3: pivot_root | 2-3 days | 9-12 days |
| 4: Integration | 2-3 days | 11-15 days |
| **Total** | | **~2 weeks** |
