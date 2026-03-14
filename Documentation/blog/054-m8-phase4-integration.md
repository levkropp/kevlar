# M8 Phase 4: Integration Testing — M8 Complete

Phase 4 validates the entire M8 feature set with a 14-subtest
integration binary and verifies full backwards compatibility.

## Integration test: mini_cgroups_ns

All 14 subtests pass:

```
TEST_PASS cgroup_mount
TEST_PASS cgroup_mkdir
TEST_PASS cgroup_move_procs
TEST_PASS cgroup_subtree_ctl
TEST_PASS cgroup_pids_max
TEST_PASS ns_uts_isolate
TEST_PASS ns_uts_unshare
TEST_PASS ns_pid_basic
TEST_PASS ns_pid_nested
TEST_PASS ns_mnt_isolate
TEST_PASS proc_cgroup
TEST_PASS proc_mountinfo
TEST_PASS proc_ns_dir
TEST_PASS systemd_boot_seq
TEST_END 14/14
```

The `systemd_boot_seq` subtest mimics systemd's actual early boot:
mount cgroup2, enable controllers, create init.scope and system.slice,
move PID 1, set pids.max — all succeed.

## PID namespace nested fork fix

`Process::fork()` now allocates a namespace-local PID when the parent
is inside a non-root PID namespace. Previously, only `clone()` with
`CLONE_NEWPID` (creating a *new* namespace) allocated ns PIDs. Forks
*within* an existing namespace were getting the global PID, making
`getpid()` return the wrong value for grandchildren.

## Full regression

- Contract tests: 28/29 PASS, 1 XFAIL (ns_uts needs root on Linux)
- musl pthreads: 14/14 on -smp 4
- glibc pthreads: 14/14 on -smp 4
- glibc hello: PASS
- mini_cgroups_ns: 14/14

## M8 summary

| Phase | Deliverable |
|-------|-------------|
| 1 | cgroups v2 hierarchy, CgroupFs, pids.max enforcement |
| 2 | UTS/PID/mount namespaces, unshare(2), sethostname(2) |
| 3 | pivot_root(2), /proc/[pid]/mountinfo, MS_PRIVATE |
| 4 | 14-subtest integration, systemd boot sequence test |

Kevlar now has the container isolation primitives needed for M9
(systemd).
