# M5 Phase 1: File Metadata and Extended I/O

Milestone 5 is about persistent storage — VirtIO block devices, ext2, and the
filesystem plumbing that real programs expect. Phase 1 tackles the low-hanging
fruit: eight syscalls that are simple to implement, frequently hit by real
software, and unblock a wide range of programs.

## The Syscalls

**statfs / fstatfs** — Filesystem statistics. Programs like `df`, package
managers, and build tools call these to check available space and filesystem
type. The implementation returns hardcoded constants for our two filesystem
types: tmpfs (`TMPFS_MAGIC = 0x01021994`) and procfs (`PROC_SUPER_MAGIC =
0x9FA0`). Path prefix matching determines which to return. Since everything
in Kevlar is currently in-memory, the "free space" numbers are synthetic but
plausible.

**statx** — The modern replacement for `stat()`. glibc has been using this by
default since 2018, so any glibc-linked program hits it immediately. The
implementation reuses the existing `INode::stat()` infrastructure, converting
our `Stat` struct into the larger `statx` format. It supports `AT_EMPTY_PATH`
(stat an fd directly) and `AT_SYMLINK_NOFOLLOW`, following the same path
resolution pattern as `newfstatat`.

One wrinkle: `FileMode` is a `#[repr(transparent)]` newtype over `u32` but
didn't expose a getter for the raw value. Rather than using unsafe transmute,
I added `FileMode::as_u32()` to the kevlar_vfs crate. Small, but keeps the
unsafe count at zero.

**utimensat** — Set file timestamps. Used by `touch`, `cp -p`, `make`, and
many other tools. Currently a stub that returns success — our tmpfs doesn't
persist timestamps, so silently accepting is the correct behavior. When ext2
arrives in Phase 6, this will need a real implementation.

**fallocate / fadvise64** — Stubs. `fallocate` preallocates disk space (tmpfs
doesn't need this). `fadvise64` is purely advisory (hints about access
patterns). Both validate the fd exists and return success.

**preadv / pwritev** — Vectored I/O at an explicit offset. These combine the
scatter/gather of `readv`/`writev` with the offset semantics of
`pread64`/`pwrite64`. The implementation iterates over the iovec array, calling
the file's `read()`/`write()` methods at a running offset. Unlike `readv`,
these don't update the file position — important for concurrent access.

## Implementation Pattern

All eight syscalls follow the same pattern established in earlier milestones:

1. Create `kernel/syscalls/<name>.rs` with the implementation
2. Add syscall numbers for both x86_64 and ARM64 in `mod.rs`
3. Add dispatch entries in the match statement
4. Add name mappings for debug output

The struct layouts (`struct statfs`, `struct statx`) must match the Linux
kernel's ABI exactly. Both are `#[repr(C)]` with carefully ordered fields.
statx in particular is large (256 bytes) with nested timestamp structs and
spare fields for future extensions.

## ARM64 Syscall Number Care

ARM64 uses the asm-generic syscall numbering, which is completely different
from x86_64. Every new syscall needs both numbers, and they must be verified
against the Linux headers to avoid conflicts with existing entries. For this
batch: statfs=43/137, fstatfs=44/138, fallocate=47/285, preadv=69/295,
pwritev=70/296, utimensat=88/280, fadvise64=223/221, statx=291/332
(arm64/x86_64).

## What's Next

Phase 2 adds inotify — the Linux file change notification API. This is what
build tools, file managers, and development servers use to watch for changes.
The implementation needs a new `InotifyInstance` (similar to EpollInstance),
VFS hooks for file creation/deletion/modification events, and proper
integration with the existing poll/epoll infrastructure.
