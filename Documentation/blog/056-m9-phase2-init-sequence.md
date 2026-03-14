# M9 Phase 2: Systemd-Compatible Init Sequence

Phase 2 adds kernel features systemd needs and validates them with a
comprehensive 25-subtest init-sequence test.

## New kernel features

- **CLOCK_BOOTTIME** (7) — alias for CLOCK_MONOTONIC, plus
  CLOCK_MONOTONIC_RAW (4), CLOCK_*_COARSE (5, 6)
- **/proc/sys/ hierarchy** — hostname, osrelease, ostype, boot_id,
  nr_open
- **/dev/kmsg** — write goes to serial log, read returns empty
- **/dev/urandom** — random bytes via rdrand
- **/dev/full** — read returns zeros, write returns ENOSPC
- **/proc/[pid]/environ** — returns empty (stub)
- **mount() NULL fstype** — flag-only mounts (MS_BIND, MS_REMOUNT)
  now handle NULL filesystem type pointer correctly
- **MS_BIND file bind mounts** — accept silently for file targets

## mini_systemd_v3: 25/25

Exercises systemd's full boot sequence in order:

```
set_child_subreaper, mount_proc_sys_dev, bind_mount_console,
remount_nosuid, tmpfs_run_systemd, set_hostname, mount_cgroup2,
cgroup_hierarchy, move_pid1_cgroup, enable_controllers,
private_socket, main_event_loop, fork_service, waitid_reap,
memfd_data_pass, close_range_exec, flock_lockfile, inotify_watch,
service_restart, shutdown_sequence, read_proc_cgroup,
clock_boottime, proc_sys_kernel, dev_kmsg, proc_environ
```

## Results

- mini_systemd_v3: 25/25
- Contract tests: 30/31 PASS, 1 XFAIL
- musl pthreads: 14/14
