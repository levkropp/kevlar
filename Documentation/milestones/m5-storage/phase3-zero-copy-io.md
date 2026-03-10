# Phase 3: Zero-Copy I/O

**Goal:** Implement efficient data transfer between file descriptors without
round-tripping through userspace buffers. Used by web servers, file copy
utilities, package managers, and shell pipelines.

## Syscalls

| Syscall | x86_64 | arm64 | Priority | Notes |
|---------|--------|-------|----------|-------|
| `sendfile` | 40 | 71 | Required | Transfer data between fds (file→socket, file→file) |
| `splice` | 275 | 76 | Required | Transfer data between fd and pipe |
| `tee` | 276 | 77 | Nice-to-have | Duplicate pipe data without consuming it |
| `copy_file_range` | 326 | 285 | Nice-to-have | In-kernel file-to-file copy |

## Design

### sendfile

```c
ssize_t sendfile(int out_fd, int in_fd, off_t *offset, size_t count);
```

The workhorse: transfers up to `count` bytes from `in_fd` to `out_fd` without
copying through userspace. If `offset` is non-NULL, reads from that offset
(doesn't update in_fd's file position); otherwise uses and updates in_fd's
current position.

Implementation strategy: since we don't have a zero-copy page cache yet, the
initial implementation can use a kernel-side buffer:

```rust
pub fn sys_sendfile(&mut self, out_fd: Fd, in_fd: Fd,
                     offset_ptr: Option<UserVAddr>, count: usize) -> Result<isize> {
    let mut buf = [0u8; 4096];  // kernel-side bounce buffer
    let mut total = 0;
    while total < count {
        let chunk = core::cmp::min(count - total, buf.len());
        let n = /* read from in_fd into buf[..chunk] */;
        if n == 0 { break; }
        /* write buf[..n] to out_fd */;
        total += n;
    }
    Ok(total as isize)
}
```

This isn't truly zero-copy, but it avoids the user↔kernel copy overhead and
matches the syscall semantics. True zero-copy (page remapping) can come later
with a proper page cache.

### splice

```c
ssize_t splice(int fd_in, off_t *off_in,
               int fd_out, off_t *off_out,
               size_t len, unsigned int flags);
```

Transfers data between a pipe and a file descriptor (at least one end must be
a pipe). Used by shell pipelines for efficient data movement.

Implementation: similar kernel-side buffer approach. The pipe's internal ring
buffer can serve as the intermediate storage.

Flags:
- `SPLICE_F_MOVE` (1) — hint, can ignore
- `SPLICE_F_NONBLOCK` (2) — don't block
- `SPLICE_F_MORE` (4) — hint that more data coming

### tee

```c
ssize_t tee(int fd_in, int fd_out, size_t len, unsigned int flags);
```

Duplicates data from one pipe to another without consuming it from the source.
Used by `tee(1)` command. Lower priority — can stub initially.

### copy_file_range

```c
ssize_t copy_file_range(int fd_in, off_t *off_in,
                        int fd_out, off_t *off_out,
                        size_t len, unsigned int flags);
```

Server-side copy between two file descriptors (both must be files, not pipes
or sockets). Useful for `cp` and package managers. Implementation is similar
to sendfile but between two regular files.

## Kernel-Side Buffer Strategy

All four syscalls benefit from a kernel-side transfer buffer. Rather than
allocating on the stack (limited) or heap (allocation overhead), use a
per-CPU or per-syscall page:

```rust
// Reuse a single page for bounce-buffer transfers.
// Safe because syscalls are not reentrant on the same CPU (no preemption
// during the copy loop).
const BOUNCE_BUF_SIZE: usize = 4096;
```

True zero-copy (remapping pages between address spaces or between the page
cache and a socket buffer) is deferred until we have a proper page cache
and VM infrastructure for it.

## Reference Sources

- FreeBSD `sys/kern/kern_sendfile.c` (BSD-2-Clause) — sendfile
- FreeBSD `sys/kern/vfs_syscalls.c` (BSD-2-Clause) — copy_file_range
- Linux sendfile(2), splice(2), tee(2), copy_file_range(2) man pages

## Testing

- `sendfile` between a regular file and /dev/null → returns correct byte count
- `sendfile` between a file and a pipe → data arrives at pipe reader
- `sendfile` with explicit offset → reads from offset, doesn't change file pos
- `splice` from pipe to file and file to pipe
- BusyBox `cp` uses sendfile internally — test with file copy
