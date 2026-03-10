# M5 Phase 2: inotify File Change Notifications

inotify is the Linux API that lets programs watch for filesystem changes —
file creation, deletion, modification, renames. Build tools, file managers,
development servers, and container runtimes all depend on it. Phase 2
implements the core inotify infrastructure and hooks it into the VFS layer.

## Architecture

The implementation follows the same `FileLike` pattern as epoll, eventfd, and
signalfd. An `InotifyInstance` is a file descriptor that:

1. Maintains a table of watches (watch descriptor → path + event mask)
2. Queues `inotify_event` structs when watched paths see matching VFS operations
3. Is readable (returns queued events in Linux wire format) and pollable
   (POLLIN when events are pending, integrating with epoll)

### Global Watch Registry

The key design decision is how VFS operations find the inotify instances that
care about them. I went with a global registry: a `SpinLock<Vec<Arc<InotifyInstance>>>`.
When any VFS operation completes (unlink, mkdir, rename), it calls
`inotify::notify()` which scans all registered instances for matching watches.

This is O(n) in the number of active inotify instances, but n is typically
tiny (1-2 per process that uses inotify). The alternative — embedding watch
references in directory inodes — would require modifying the `Directory` trait
across all filesystem implementations, which is far more invasive for the same
practical performance.

### Path-Based Matching

Linux's inotify tracks watches by inode, but Kevlar uses path-based matching
for simplicity. A watch on `/tmp` will match events where the directory path
is `/tmp`. This works correctly for the common case (watching a directory for
child events) and avoids the complexity of inode lifecycle tracking.

The tradeoff: hardlinks and bind mounts could cause missed events. Since Kevlar
doesn't yet have persistent storage or bind mounts, this is a non-issue today.

## Wire Format

Reading from an inotify fd returns packed `struct inotify_event` structures:

```
┌─────────┬──────────┬──────────┬─────────┬────────────────┐
│ wd (4B) │ mask(4B) │cookie(4B)│ len(4B) │ name (len, NUL)│
└─────────┴──────────┴──────────┴─────────┴────────────────┘
```

The name field is NUL-terminated and padded to 4-byte alignment. Multiple
events can be returned in a single `read()` call. The serialization uses
`UserBufWriter` to write directly into userspace buffers, same as eventfd
and signalfd.

## VFS Hooks

Three syscall handlers got inotify hooks:

- **unlink** → `IN_DELETE` on the parent directory
- **mkdir** → `IN_CREATE` on the parent directory
- **rename** → paired `IN_MOVED_FROM` + `IN_MOVED_TO` with a shared cookie

The rename hook is the most interesting: both events share a monotonically
increasing cookie value so userspace can correlate the "moved from" and
"moved to" halves of a rename operation.

I deliberately skipped hooks on the hot paths (open, close, read, write) for
now. These would add overhead to every I/O operation for a feature most
programs don't use. They can be added later behind a check — if the global
registry is empty, the hook is a single atomic load and branch-not-taken.

## Blocking and Nonblock

The read path follows the standard pattern from eventfd/signalfd:

1. **Fast path:** Lock the event queue, drain events into the user buffer,
   return immediately if any events were available
2. **Nonblock:** If `IN_NONBLOCK` was set on `inotify_init1`, return `EAGAIN`
3. **Slow path:** `POLL_WAIT_QUEUE.sleep_signalable_until()` — sleep until
   events arrive, then drain and return

The `notify()` function calls `POLL_WAIT_QUEUE.wake_all()` after queuing
events, which wakes any blocked readers and any epoll instances watching
the inotify fd.

## What's Next

Phase 3 implements zero-copy I/O: `sendfile`, `splice`, `tee`, and
`copy_file_range`. These syscalls move data between file descriptors without
copying through userspace, and are heavily used by web servers, file copy
utilities, and container runtimes.
