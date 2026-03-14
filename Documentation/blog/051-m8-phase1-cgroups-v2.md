# M8 Phase 1: cgroups v2 Unified Hierarchy

Phase 1 implements the cgroups v2 filesystem with a real hierarchy,
control files, and pids.max enforcement.

## Architecture

A new `kernel/cgroups/` module provides:

- **CgroupNode** — tree node with name, parent, children, member PIDs,
  controller limits (pids_max, memory_max, cpu_max), and
  subtree_control bitflags.
- **CgroupFs** — implements FileSystem, returns a CgroupDir as root.
- **CgroupDir** — implements Directory with dynamic lookup for child
  cgroups and control files (cgroup.procs, cgroup.controllers,
  cgroup.subtree_control, etc.), plus create_dir/rmdir for hierarchy
  management.
- **CgroupControlFile** — implements FileLike with read/write for
  each control file type.

## What works

- `mount -t cgroup2 none /sys/fs/cgroup` produces a real cgroup tree
- `mkdir` creates child cgroups with inherited controllers
- Writing a PID to `cgroup.procs` moves the process
- `cgroup.controllers` lists available controllers (cpu, memory, pids)
- `cgroup.subtree_control` accepts `+pids -cpu` format
- `pids.max` is enforced: fork returns EAGAIN when the subtree PID
  count reaches the limit
- `memory.max` and `cpu.max` are readable/writable stubs
- `/proc/[pid]/cgroup` returns `0::/<cgroup_path>`

## Process integration

Process gains a `cgroup: Option<Arc<CgroupNode>>` field. Fork
inherits the parent's cgroup and registers the child PID in the
cgroup's member list. Before allocating a PID, fork checks pids.max
limits by walking up the cgroup tree.

## Contract test

The `cgroup_basic` contract test verifies:
- `/proc/self/cgroup` returns valid `0::/<path>` format
- `/proc/filesystems` lists `cgroup2`

The full cgroup hierarchy test (mount, mkdir, procs, pids.max) runs
as an integration test since it requires root/PID 1 privileges that
the contract test runner doesn't have.

## Results

27/27 contract tests pass, zero divergences.
