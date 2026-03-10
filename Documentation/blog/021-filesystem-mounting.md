# Blog 021: Filesystem Mounting & /proc Improvements

**M4 Phase 4 — mount/umount2, dynamic /proc, /sys stubs**

systemd expects to mount filesystems at boot — proc on /proc, sysfs on /sys,
tmpfs on /run, cgroup2 on /sys/fs/cgroup. It also reads /proc extensively:
/proc/self/stat, /proc/1/cmdline, /proc/meminfo, /proc/mounts. Phase 4
implements all of this.

## mount(2) and umount2(2)

The `sys_mount` handler reads the target path and filesystem type string from
userspace, then dispatches on fstype:

- **proc** — uses the global `PROC_FS` singleton
- **sysfs** — uses the global `SYS_FS` singleton
- **tmpfs** — creates a fresh `TmpFs`
- **devtmpfs/devpts** — silently succeeds (our devfs is always mounted)
- **cgroup2/cgroup** — creates an empty tmpfs (stub)

If the target directory doesn't exist, mount auto-creates it (like `mkdir -p`).
After mounting, the entry is recorded in a global `MountTable` so /proc/mounts
can report it.

`sys_umount2` just removes the entry from the mount table. We don't actually
detach the VFS mount — systemd rarely unmounts at runtime and the VFS layer
doesn't support it yet.

## MountTable

A simple `SpinLock<VecDeque<MountEntry>>` tracking `(fstype, mountpoint)` pairs.
Initialized at boot with the known mounts (rootfs on /, proc on /proc, devtmpfs
on /dev, tmpfs on /tmp). `format_mounts()` generates Linux-compatible output for
/proc/mounts:

```
rootfs / rootfs rw 0 0
proc /proc proc rw 0 0
devtmpfs /dev devtmpfs rw 0 0
tmpfs /tmp tmpfs rw 0 0
```

## Dynamic /proc

Previously, /proc was a flat tmpfs with a few static files. Now `ProcRootDir`
intercepts lookups:

1. **"self"** — returns a `ProcSelfSymlink` that resolves to `/proc/<current_pid>`
2. **Numeric names** — parses as PID, returns a `ProcPidDir` generated on the fly
3. **Everything else** — delegates to the static tmpfs (mounts, meminfo, etc.)

### Per-PID directories (/proc/[pid]/)

`ProcPidDir` provides five entries:

| File | Content |
|------|---------|
| stat | 52-field format: `pid (comm) S ppid ...` |
| status | Key-value: Name, State, Pid, PPid, Uid, Gid |
| cmdline | NUL-separated argv (spaces → NUL bytes) |
| comm | Process name + newline |
| exe | Symlink to argv0 (readlink) |

All entries are synthesized on read from the live process table via
`Process::find_by_pid()`. No data is cached.

### System-wide files

| File | Source |
|------|--------|
| /proc/mounts | MountTable::format_mounts() |
| /proc/filesystems | Static list: proc, sysfs, tmpfs, devtmpfs, cgroup2 |
| /proc/cmdline | "kevlar\n" |
| /proc/stat | CPU time from monotonic clock, process counts |
| /proc/meminfo | MemTotal/MemFree from page allocator stats |
| /proc/version | "Kevlar version 0.1.0 (rustc) #1 SMP\n" |

## /sys stubs

systemd probes /sys at early boot looking for cgroup controllers, device classes,
and kernel parameters. `SysFs` wraps a TmpFs with empty directories:

- /sys/fs/cgroup
- /sys/class
- /sys/devices
- /sys/bus
- /sys/kernel

This is enough for systemd to see sysfs is mounted and continue without errors.
The directories are empty — no actual sysfs attributes yet.

## Syscall summary

| Syscall | x86_64 | ARM64 |
|---------|--------|-------|
| mount | 165 | 40 |
| umount2 | 166 | 39 |

Total implementation: ~900 lines across 10 files. The /proc infrastructure is
the most complex piece — the dynamic root directory pattern will extend easily
as we add more per-PID entries (fd/, maps, etc.) in later phases.
