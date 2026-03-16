# Phase 1: File Metadata & Extended Operations

**Goal:** Implement the commonly-needed file metadata and extended I/O syscalls
that real programs expect. These are easy wins — simple to implement, frequently
hit, and unblock a wide range of software.

## Syscalls

| Syscall | x86_64 | arm64 | Priority | Notes |
|---------|--------|-------|----------|-------|
| `statfs` | 137 | 43 | Required | Filesystem statistics (df, package managers) |
| `fstatfs` | 138 | 44 | Required | statfs on fd |
| `utimensat` | 280 | 88 | Required | Set file timestamps (touch, cp -p, make) |
| `statx` | 332 | 291 | Required | Modern stat (glibc uses by default) |
| `fallocate` | 285 | 47 | Stub | Preallocate space (accept, return success) |
| `fadvise64` | 221 | 223 | Stub | Hint about access patterns (accept, ignore) |
| `preadv` | 295 | 69 | Required | Vectored read at offset |
| `pwritev` | 296 | 70 | Required | Vectored write at offset |

## Design

### statfs / fstatfs

Returns filesystem statistics in `struct statfs`:

```c
struct statfs {
    long    f_type;     // filesystem type (EXT2_SUPER_MAGIC, TMPFS_MAGIC, etc.)
    long    f_bsize;    // block size
    long    f_blocks;   // total blocks
    long    f_bfree;    // free blocks
    long    f_bavail;   // free blocks for unprivileged users
    long    f_files;    // total inodes
    long    f_ffree;    // free inodes
    fsid_t  f_fsid;     // filesystem ID
    long    f_namelen;  // max filename length
    long    f_frsize;   // fragment size
    long    f_flags;    // mount flags
};
```

Implementation: add a `statfs()` method to the `Directory` trait (or a new
`Filesystem` trait). Each filesystem type returns its own constants:
- tmpfs: `f_type = 0x01021994` (TMPFS_MAGIC), `f_bsize = 4096`
- procfs: `f_type = 0x9FA0` (PROC_SUPER_MAGIC)
- ext2 (Phase 6): `f_type = 0xEF53` (EXT2_SUPER_MAGIC), real block counts

For the syscall, resolve the path to its filesystem and call `statfs()`.

### utimensat

Sets file access and modification timestamps:

```c
int utimensat(int dirfd, const char *pathname,
              const struct timespec times[2], int flags);
```

- `times[0]` = access time, `times[1]` = modification time
- `UTIME_NOW` (0x3FFFFFFF) = set to current time
- `UTIME_OMIT` (0x3FFFFFFE) = don't change this timestamp
- `AT_SYMLINK_NOFOLLOW` flag = operate on symlink itself

Implementation: add `atime` and `mtime` fields to `Stat`, and a `set_times()`
method to `FileLike` / `Directory`. tmpfs stores timestamps in the inode.
Initially, all timestamps can default to boot time; `utimensat` updates them.

### statx

Modern replacement for stat with more fields:

```c
struct statx {
    __u32 stx_mask;        // which fields are valid
    __u32 stx_blksize;
    __u64 stx_attributes;
    __u32 stx_nlink;
    __u32 stx_uid, stx_gid;
    __u16 stx_mode;
    __u64 stx_ino;
    __u64 stx_size;
    __u64 stx_blocks;
    struct statx_timestamp stx_atime, stx_btime, stx_ctime, stx_mtime;
    // ... more fields
};
```

Implementation: mostly a superset of `stat()`. Fill from existing `Stat` struct
plus the new timestamp fields. `stx_mask` indicates which fields are valid —
we return what we have and set the mask accordingly.

### preadv / pwritev

Vectored I/O at an explicit offset (combines pread + readv):

```c
ssize_t preadv(int fd, const struct iovec *iov, int iovcnt, off_t offset);
ssize_t pwritev(int fd, const struct iovec *iov, int iovcnt, off_t offset);
```

Implementation: iterate over the iovec array, calling `read()`/`write()` for
each segment at the cumulative offset. Similar to existing `readv`/`writev`
but with explicit offset (doesn't update file position).

### Stubs

- `fallocate`: Accept and return 0 (tmpfs doesn't need preallocation)
- `fadvise64`: Accept and return 0 (advisory, no effect needed)

## Reference Sources

- Linux man pages: statfs(2), utimensat(2), statx(2), preadv(2)

## Testing

- `statfs /` returns TMPFS_MAGIC with sensible values
- `touch -t` modifies timestamps, `stat` shows updated values
- `statx` returns valid data for regular files, directories, symlinks
- `preadv`/`pwritev` work correctly with multiple iovec segments
