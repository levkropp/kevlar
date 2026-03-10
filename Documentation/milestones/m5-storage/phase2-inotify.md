# Phase 2: inotify

**Goal:** Implement Linux inotify — file/directory change notification via a
readable file descriptor. Integrates with epoll for event-driven I/O.

## Syscalls

| Syscall | x86_64 | arm64 | Priority | Notes |
|---------|--------|-------|----------|-------|
| `inotify_init1` | 294 | 26 | Required | Create inotify fd with IN_CLOEXEC/IN_NONBLOCK |
| `inotify_add_watch` | 254 | 27 | Required | Add watch on path with event mask |
| `inotify_rm_watch` | 255 | 28 | Required | Remove a watch |

## Design

### InotifyInstance

An inotify fd is a `FileLike` object that:
- Maintains a table of watches (wd → path + event mask)
- Queues `inotify_event` structs when watched files change
- Is readable (returns queued events) and pollable (POLLIN when events pending)

```rust
pub struct InotifyInstance {
    watches: SpinLock<Vec<InotifyWatch>>,
    events: SpinLock<VecDeque<InotifyEvent>>,
    next_wd: AtomicI32,
}

struct InotifyWatch {
    wd: i32,
    path: PathBuf,
    mask: u32,
}

struct InotifyEvent {
    wd: i32,
    mask: u32,      // IN_CREATE, IN_DELETE, etc.
    cookie: u32,    // for IN_MOVED_FROM/TO pairing
    name: String,   // filename (for directory watches)
}
```

### Event Mask Constants

```c
IN_ACCESS        0x00000001  // file accessed
IN_MODIFY        0x00000002  // file modified
IN_ATTRIB        0x00000004  // metadata changed
IN_CLOSE_WRITE   0x00000008  // writable fd closed
IN_CLOSE_NOWRITE 0x00000010  // read-only fd closed
IN_OPEN          0x00000020  // file opened
IN_MOVED_FROM    0x00000040  // file moved out of watched dir
IN_MOVED_TO      0x00000080  // file moved into watched dir
IN_CREATE        0x00000100  // file/dir created in watched dir
IN_DELETE        0x00000200  // file/dir deleted from watched dir
IN_DELETE_SELF   0x00000400  // watched file/dir deleted
IN_MOVE_SELF     0x00000800  // watched file/dir moved
```

### Notification Hookpoints

inotify needs to be notified when filesystem operations happen. Add hooks in
the VFS layer (not in individual filesystems):

- `vfs_create()` / `vfs_mkdir()` → IN_CREATE
- `vfs_unlink()` / `vfs_rmdir()` → IN_DELETE
- `vfs_rename()` → IN_MOVED_FROM + IN_MOVED_TO (same cookie)
- `vfs_open()` → IN_OPEN
- `vfs_close()` → IN_CLOSE_WRITE or IN_CLOSE_NOWRITE
- `vfs_write()` → IN_MODIFY
- `vfs_chmod()` / `vfs_utimensat()` → IN_ATTRIB

Implementation strategy: a global registry of active watches. Each VFS
operation checks the registry for matching watches and queues events.
Use a simple path-prefix match initially (not inode-based tracking).

### Wire Format

`read()` on an inotify fd returns packed `struct inotify_event`:

```c
struct inotify_event {
    int      wd;       // watch descriptor
    uint32_t mask;     // event mask
    uint32_t cookie;   // for rename pairing
    uint32_t len;      // length of name (including NUL padding to alignment)
    char     name[];   // filename (NUL-terminated, padded to 4-byte alignment)
};
```

Multiple events can be returned in a single `read()` call. The fd is
non-blocking if IN_NONBLOCK was set on inotify_init1.

### Epoll Integration

InotifyInstance implements `poll()` returning `POLLIN` when the event queue
is non-empty. This lets programs `epoll_wait` for file changes alongside
other event sources (timers, signals, sockets).

## Scope

**In scope:**
- tmpfs/initramfs file operations trigger inotify events
- Directory watches (watch a dir, get events for files within it)
- Single-file watches
- Epoll integration

**Deferred:**
- ext2 filesystem events (Phase 6 is read-only anyway)
- IN_EXCL_UNLINK, IN_MASK_ADD, IN_ONESHOT (advanced flags)
- Recursive directory watching (programs use multiple add_watch calls)

## Reference Sources

- FreeBSD `sys/compat/linux/linux_inotify.c` (BSD-2-Clause) — linuxulator
- Linux inotify(7) man page — interface specification
- OSDev wiki — inotify implementation notes

## Testing

- Create inotify fd, add watch on `/tmp`, create a file → IN_CREATE event
- Modify watched file → IN_MODIFY event
- Delete watched file → IN_DELETE event
- inotify fd readable via epoll_wait
- Non-blocking read returns EAGAIN when no events pending
