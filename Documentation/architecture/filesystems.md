# Filesystems

## VFS Layer

Kevlar's VFS (`libs/kevlar_vfs/`) provides a uniform interface over all filesystems.
The core types are:

### INode

```rust
pub enum INode {
    File(Arc<dyn FileLike>),
    Dir(Arc<dyn Directory>),
    Symlink(Arc<dyn Symlink>),
}
```

All filesystem operations go through these traits. The kernel holds `INode` values;
it never calls filesystem-specific code directly.

### PathComponent

`PathComponent` is a node in the path tree. Each path component records:
- Its name (the last segment of the path)
- A reference to its parent `PathComponent`
- The `INode` it resolves to

Path resolution traverses the tree from the process's root or CWD, following
`PathComponent` links at each segment. Symlinks are followed (up to `MAXSYMLINKS`
depth). Mount points are resolved via the mount table before each lookup.

### OpenedFileTable

A per-process table mapping file descriptors (integers) to `Arc<dyn FileLike>`.
The table uses `SpinLock` for concurrent access. `dup`, `dup2`, `close`, `open`,
and `socket` all manipulate this table.

`O_CLOEXEC` is tracked per-fd and respected on `execve`.

## Filesystem Implementations

### initramfs

A read-only CPIO newc archive embedded in the kernel image. Parsed at boot by
`services/kevlar_initramfs`. All files are extracted into memory; the initramfs
parser itself is `#![forbid(unsafe_code)]`.

### tmpfs

An in-memory read-write filesystem (`services/kevlar_tmpfs`). Supports regular files,
directories, symlinks, hard links, and all standard POSIX file operations. Used for `/`,
`/tmp`, and all runtime-created files.

### procfs

Mounted at `/proc`. Provides process and system introspection:

| Path | Content |
|---|---|
| `/proc/[pid]/status` | Name, state, PID, PPID, UID/GID, VM size, signal masks |
| `/proc/[pid]/maps` | Virtual memory map (one VMA per line) |
| `/proc/[pid]/fd/` | Open file descriptors as symlinks |
| `/proc/[pid]/cmdline` | Process argv, NUL-separated |
| `/proc/self` | Symlink to the calling process's `/proc/[pid]` |
| `/proc/cpuinfo` | CPU vendor, model, frequency, flags |
| `/proc/uptime` | Seconds since boot |
| `/proc/loadavg` | Load averages and process counts |

### devfs

Mounted at `/dev`. Provides device nodes:

| Node | Description |
|---|---|
| `/dev/null` | Discards all writes; reads return EOF |
| `/dev/zero` | Reads return zero bytes |
| `/dev/urandom` | Reads return random bytes (RDRAND/RDSEED) |
| `/dev/console` | Serial console TTY (process 0's stdio) |
| `/dev/tty` | The controlling terminal of the calling process |
| `/dev/pts/N` | Pseudo-terminal slave devices |

## Mount Namespace

`mount(2)` adds entries to the mount table. Each entry maps a path prefix to an
`INode` (the mounted filesystem root). During path resolution, the mount table is
checked at each component to detect mount points.

`umount(2)` removes the topmost mount at a path. Bind mounts (`MS_BIND`) duplicate
a subtree at a new path.

## inotify

The inotify subsystem (`kernel/fs/inotify.rs`) watches paths for filesystem events.
A global watch registry maps watched paths to a list of `InotifyInstance` handles.
VFS operations (`create`, `unlink`, `rename`, `open`, `close`, `modify`) call into
the registry to deliver events.

Events are queued in the `InotifyInstance`'s ring buffer and readable via `read(2)`.
`poll(2)` / `epoll` work on inotify file descriptors.

## File Locking and Metadata

Supported metadata operations: `stat`, `fstat`, `lstat`, `newfstatat`, `statx`,
`statfs`, `fstatfs`, `utimensat`, `fallocate`, `fadvise64`.

Advisory file locking (`flock`) is implemented. Mandatory locking is not.
