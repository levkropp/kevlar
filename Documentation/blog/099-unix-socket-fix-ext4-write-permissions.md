# Blog 099: Unix socket stack overflow fix + ext4 extent writes + chown/chmod — 118/118 PASS

**Date:** 2026-03-21
**Milestone:** M10 Alpine Linux

## Context

Three major gaps stood between Kevlar and booting real ext4-based distros:

1. **2 XFAIL contract tests** (`sockets.accept4_flags`, `sockets.unix_stream`) —
   kernel stack corruption during fork+accept+connect
2. **ext4 extent writes** — existing ext4 files were read-only; new files used
   legacy block pointers even on ext4 filesystems
3. **chown/chmod stubs** — `fchmod`, `fchown`, `fchownat` all returned `Ok(0)`
   without doing anything; `getegid` returned constant 0

This session fixed all three, reaching **118/118 contract tests passing**
with **0 benchmark regressions**.

---

## Fix 1: Unix socket stack overflow (116/118 → 118/118)

### Root cause

`StreamInner` in `kernel/net/unix_socket.rs` contained a
`RingBuffer<u8, 16384>` — a **16KB inline array**. When
`Arc::new(SpinLock::new(StreamInner { ... }))` was called during `connect()`,
Rust constructed the 16KB struct on the **8KB syscall_stack** before moving
it to the heap. The overflow wrote zeros into adjacent physical memory.

When PID 1's `syscall_stack` happened to be allocated just below PID 2's
stack in physical memory, the overflow corrupted PID 1's saved kernel
context. On the next context switch to PID 1, `do_switch_thread` popped
all-zeros and jumped to `rip=0x0`.

This is the same class of bug as the pipe stack overflow fixed in blog 098
(`PipeInner` with 65KB `RingBuffer` on 16KB kernel stack).

### Investigation path

The blog 094 investigation had ruled out `zero_page()`, `alloc_page()` cache,
`OwnedPages` refcount, and ghost fork — all allocator-level checks. The
actual corruption was a **direct stack pointer overflow**, bypassing all
allocator instrumentation. The key insight was recognizing that
`StreamInner`'s 16KB `RingBuffer` exceeds the 8KB `syscall_stack`, exactly
matching the pipe overflow pattern.

### Fix

Allocate `StreamInner` via `alloc_zeroed` + `Box::from_raw` (identical
pattern to the pipe fix). Changed `UnixStream.tx`/`rx` from
`Arc<SpinLock<StreamInner>>` to `Arc<SpinLock<Box<StreamInner>>>` so the
`SpinLock` only holds a pointer (8 bytes) on the stack.

All fields are correct when zeroed: `RingBuffer` (rp=0, wp=0, full=false),
`Option<VecDeque>` (None = 0), `bool` (false = 0).

---

## Fix 2: ext4 extent tree write support

### Problem

Real ext4 filesystems (created by `mkfs.ext4`) use extent trees for all
files. Kevlar could read these files but writing returned `ENOSPC`:

```rust
if use_extents {
    // Can't extend extent-based files with block pointers.
    return Err(Error::new(Errno::ENOSPC));
}
```

Additionally, new files were always created with legacy block pointers,
and `free_file_blocks()` misinterpreted extent tree data as block pointers,
corrupting bitmaps on unlink/rmdir.

### Implementation

All changes in `services/kevlar_ext2/src/lib.rs` (~300 lines added):

**Serialization:** Added `serialize()` to `ExtentHeader`, `Extent`,
`ExtentIdx`. Added `Extent::new()`, `ExtentIdx::new()` constructors.

**Goal-based allocation:** `alloc_block_near(goal)` scans from the goal's
block group and bit position first, maximizing physical contiguity. Uses
`find_free_bit_from(bitmap, start_bit, max_bits)` with wraparound.

**Extent insertion (`alloc_extent_block`):** The core write function:
1. Tries to **extend** an adjacent extent (hot path for sequential writes —
   allocates contiguous physical block and increments `ext.len`)
2. Tries to **prepend** (reverse-sequential writes)
3. **Inserts** a new single-block extent at sorted position
4. If leaf is full, **splits** the root (depth 0 → 1)

**Tree splitting (`split_and_insert`):** When the root's 4 extent slots
are full, allocates two disk-block leaf nodes, distributes extents between
them, and rewrites the root as depth-1 with two `ExtentIdx` entries.
Each disk-block leaf holds 340 extents, so this rarely triggers again.

**Extent-aware free (`free_extent_blocks`):** Recursive tree walker that
frees all physical blocks at leaf level, then frees internal node blocks.
Fixes the critical `free_file_blocks` bug for extent inodes.

**Truncate(0) fast path:** For `O_TRUNC` on extent files, frees all extent
blocks and reinitializes an empty depth-0 tree.

**New file creation:** `create_file`, `create_dir`, `create_symlink` now
set `EXT4_EXTENTS_FL` and initialize extent tree roots on ext4 filesystems.

### Key numbers

| Metric | Value |
|--------|-------|
| Root extent slots | 4 (60 bytes - 12 header = 48, 48/12 = 4) |
| Disk leaf slots | 340 ((4096 - 12) / 12) |
| Max contiguous extent | 32768 blocks = 128MB |
| Depth-0 coverage (4 contiguous extents) | 512MB |
| Depth-1 coverage | 4 × 340 = 1360 extents — effectively unlimited |

---

## Fix 3: File permissions + chown/chmod

### Changes

**VFS trait layer (`libs/kevlar_vfs/src/inode.rs`):**
- Added `chown(uid, gid)` to `FileLike`, `Directory`, and `INode` traits

**tmpfs (`services/kevlar_tmpfs/src/lib.rs`):**
- Added `uid: SpinLock<UId>`, `gid: SpinLock<GId>` to `Dir` and `File`
- `stat()` now returns mutable uid/gid; `chown()` updates them

**Syscalls:**
- `fchmod` / `fchmodat` / `fchownat`: replaced stubs with real implementations
- New `chown.rs`: `sys_chown`, `sys_fchown` — resolve path/fd, call `inode.chown()`
- `access` / `faccessat`: now pass mode argument and use `check_access()` DAC helper

**Permission checking (`kernel/fs/permission.rs`):**
- Root (euid=0) bypasses all checks (preserves existing behavior)
- Non-root: checks owner/group/other permission bits

**Bug fixes:**
- `getegid`: returned constant 0, now returns `process.egid()`
- Initramfs: preserved uid/gid from cpio headers (was discarding as `_uid`/`_gid`)

**Constants (`libs/kevlar_vfs/src/stat.rs`):**
- Added S_ISUID, S_ISGID, S_ISVTX, S_I{RWX}{USR,GRP,OTH}, S_IFIFO, S_IFSOCK
- Added `UId::as_u32()`, `GId::as_u32()` accessors

**Device dispatch:**
- Added `/dev/random` (alias for urandom, matches Linux 5.18+)

---

## Summary

| Change | Impact |
|--------|--------|
| Unix socket stack overflow fix | 116/118 → 118/118 PASS |
| ext4 extent write support | Real ext4 rootfs images now writable |
| chown/chmod/fchmod/fchown | Multi-user file ownership works |
| getegid bug fix | Returns actual egid instead of 0 |
| Initramfs uid/gid preservation | Correct ownership from cpio |
| /dev/random | Common device alias available |
| Permission checking (check_access) | DAC infrastructure ready for non-root |

**Contract tests:** 118/118 PASS, 0 XFAIL, 0 FAIL
**Benchmarks:** 44/44 complete, 0 REGRESSION
