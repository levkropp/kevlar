# Phase 4: Filesystem Mounting

**Goal:** Implement mount/umount2 and enough of /proc and /sys for systemd to
boot. systemd mounts these pseudo-filesystems early in its startup sequence.

**Prerequisite:** None (independent of epoll work).

## Syscalls

| Syscall | Number | Priority | Notes |
|---------|--------|----------|-------|
| `mount` | 165 | Required | Mount filesystems by type |
| `umount2` | 166 | Required | Unmount with MNT_DETACH flag |

## What systemd Mounts

systemd's `mount-setup.c` mounts these at early boot:

| Mountpoint | Type | Required | Notes |
|------------|------|----------|-------|
| `/proc` | proc | Yes | Process information, critical |
| `/sys` | sysfs | Yes | Device/driver info, can stub most |
| `/dev` | devtmpfs | Yes | We already have devfs |
| `/dev/shm` | tmpfs | Yes | Shared memory |
| `/run` | tmpfs | Yes | Runtime state |
| `/sys/fs/cgroup` | cgroup2 | Maybe | Service resource control; can stub |
| `/dev/pts` | devpts | Maybe | PTY allocation; can defer |

## Design

### Mount Table

```rust
struct MountEntry {
    source: String,         // device or "none"
    mountpoint: PathBuf,    // where it's mounted
    fstype: String,         // "proc", "sysfs", "tmpfs", etc.
    flags: MountFlags,      // MS_RDONLY, MS_NOSUID, etc.
}

/// Global mount table. Simple Vec for now — no mount namespaces.
static MOUNT_TABLE: SpinLock<Vec<MountEntry>> = SpinLock::new(Vec::new());
```

### mount(2) Implementation

```rust
fn sys_mount(source, target, fstype, flags, data) -> Result<isize> {
    match fstype {
        "proc"    => mount_procfs(target)?,
        "sysfs"   => mount_sysfs(target)?,
        "tmpfs"   => mount_tmpfs(target, data)?,
        "devtmpfs"=> Ok(0),  // our devfs is always mounted
        _         => Err(Errno::ENODEV),
    }
    MOUNT_TABLE.lock().push(MountEntry { ... });
    Ok(0)
}
```

### /proc Improvements

Current state: Kevlar has a basic procfs but it's incomplete. systemd reads:

| Path | Priority | Content |
|------|----------|---------|
| `/proc/self/` | Required | Symlink to `/proc/[pid]/` |
| `/proc/[pid]/stat` | Required | Process status line |
| `/proc/[pid]/status` | Required | Human-readable status |
| `/proc/[pid]/cmdline` | Required | NUL-separated argv |
| `/proc/[pid]/fd/` | Nice-to-have | Directory of open fds |
| `/proc/[pid]/exe` | Nice-to-have | Symlink to executable |
| `/proc/cmdline` | Required | Kernel command line |
| `/proc/mounts` | Required | Mounted filesystems |
| `/proc/filesystems` | Required | Supported fs types |
| `/proc/stat` | Required | System-wide statistics |
| `/proc/meminfo` | Required | Memory usage |
| `/proc/version` | Nice-to-have | Kernel version string |
| `/proc/1/` | Required | PID 1 (systemd) info |

### /sys Stubs

systemd probes sysfs extensively but can survive with minimal content:

| Path | What systemd wants | Our approach |
|------|-------------------|--------------|
| `/sys/` | Directory exists | Empty dir |
| `/sys/fs/cgroup/` | cgroup2 mount point | Empty or stub |
| `/sys/class/` | Device classes | Empty |
| `/sys/devices/` | Device tree | Empty |

Most sysfs reads are for hardware discovery and can return ENOENT without
crashing systemd. Focus on making the mount succeed and the directory exist.

## Files to Create/Modify

- `kernel/syscalls/mount.rs` (NEW) — mount/umount2 handlers
- `kernel/fs/mount.rs` (NEW or extend) — MountTable, mount logic
- `kernel/fs/procfs/` (extend) — Per-PID directories, /proc/mounts, etc.
- `kernel/fs/sysfs.rs` (NEW) — Minimal sysfs stub (just directories)
- `kernel/syscalls/mod.rs` — dispatch entries

## Integration Test

```c
// Test: mount tmpfs, write a file, read it back
mkdir("/mnt", 0755);
mount("none", "/mnt", "tmpfs", 0, NULL);

int fd = open("/mnt/test", O_CREAT | O_WRONLY, 0644);
write(fd, "hello", 5);
close(fd);

fd = open("/mnt/test", O_RDONLY);
char buf[6] = {0};
read(fd, buf, 5);
assert(strcmp(buf, "hello") == 0);

// Test: /proc/self/stat exists
fd = open("/proc/self/stat", O_RDONLY);
assert(fd >= 0);
int n = read(fd, buf, sizeof(buf));
assert(n > 0);

// Test: /proc/mounts lists our mount
fd = open("/proc/mounts", O_RDONLY);
char mbuf[256];
read(fd, mbuf, sizeof(mbuf));
assert(strstr(mbuf, "/mnt") != NULL);

umount2("/mnt", 0);
printf("TEST_PASS mount\n");
```

## Reference

- FreeBSD: `sys/kern/vfs_mount.c` (mount infrastructure)
- Linux: `fs/namespace.c` (mount), `fs/proc/` (procfs)
- Linux man pages: mount(2), proc(5)

## Estimated Complexity

~600-800 lines. mount/umount themselves are simple dispatch. The bulk of the
work is in /proc per-PID directories and /proc/mounts generation.
