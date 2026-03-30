# Blog 130: ext4 write hardening — 7 fixes, ordered barriers, Alpine OpenRC boots

**Date:** 2026-03-30
**Milestone:** M10 Alpine Linux — Phase 4 (ext4 Write Support)

## Summary

Major ext4 write support overhaul: 4 correctness fixes, 3 crash consistency
features, and a file timestamp fix. Result: Alpine Linux with OpenRC init
system boots from a writable ext4 root filesystem for the first time.

## Phase 1: Correctness Fixes

### Cross-directory rename (was returning EXDEV)

`Ext2Dir::rename()` returned `EXDEV` for all cross-directory renames on the
same filesystem. Every package manager does cross-dir renames
(`/tmp/.apk.XXXX` -> `/etc/apk/...`), so this blocked real-world usage.

Fix: downcast `new_dir` via the `Downcastable` trait's `as_any()` method
(deref through `Arc<dyn Directory>` to dispatch via vtable, not the blanket
impl on Arc itself). Verify same filesystem via `Arc::ptr_eq`. Then:
remove from old parent, add to new parent, update `..` entry for directory
renames, adjust link counts.

### Extent tree truncation (stale entries after non-zero truncate)

Truncating an extent-based file to non-zero size freed physical blocks but
left extent entries pointing to the freed blocks. When those blocks were
reallocated to another file, both files' extent trees referenced the same
blocks — silent corruption.

Fix: `truncate_extent_tree()` walks the extent tree (depth-0 and depth-1)
and removes or shrinks extents past the new block count. Uses
`prune_leaf_extents()` for each node, compacting index entries for
multi-level trees.

### chown persisted to disk

Neither `Ext2File` nor `Ext2Dir` implemented `chown()` — fell through to
the VFS default (silent no-op). Package managers set ownership that silently
disappeared on next read.

### sync(2) flushes dirty cache

`SYS_SYNC` was a no-op with the comment "we write through on every
operation" — but the ext4 implementation uses a 32MB dirty write-back cache.
Now calls `kevlar_ext2::sync_all()` via a global `MOUNTED_FS` reference.

## Phase 2: Crash Consistency

### virtio-blk flush (VIRTIO_BLK_T_FLUSH)

`BlockDevice::flush()` was a no-op. Implemented proper `VIRTIO_BLK_T_FLUSH`
(type=4) with a 2-descriptor chain (header + status, no data). Write
ordering guarantees are now enforceable.

### Ordered-mode write barriers

`flush_dirty()` now classifies dirty blocks as data vs metadata using
`is_metadata_block()` (checks superblock, GDT, bitmap, and inode table
ranges from the block group descriptors). Flushes in two passes:

1. Data blocks → `device.flush()` barrier
2. Metadata blocks → `device.flush()` barrier

This gives ext3 "ordered" mode semantics: metadata never points to blocks
containing stale or wrong data after a crash.

### Superblock dirty/clean flag

Writes `s_state=0` (dirty) on mount, `s_state=EXT2_VALID_FS` (1) on clean
`flush_all`. Lets `e2fsck` detect unclean shutdowns without a full journal.

## File creation timestamps

All new files, directories, and symlinks were created with
`atime=ctime=mtime=0` (Unix epoch 1970). The ext4 service crate now uses
`kevlar_vfs::vfs_clock_secs()` — the VFS clock that the kernel timer
subsystem updates from the CMOS RTC on every tick.

## Alpine OpenRC Boot Result

With these fixes, Alpine Linux 3.21 with OpenRC 0.55.1 boots from a 1GB
ext4 root filesystem:

```
OpenRC 0.55.1 is starting up Linux 6.19.8 (x86_64)
 * Mounting /run ...                                              [ ok ]
 * Caching service dependencies ...                               [ ok ]
 * Remounting devtmpfs on /dev ...                                [ ok ]
 * Mounting /dev/shm ...                                          [ ok ]
 * Checking local filesystems ...                                 [ ok ]
 * Remounting filesystems ...                                     [ ok ]
 * Mounting local filesystems ...                                 [ ok ]
 * Configuring kernel parameters ...                              [ !! ]
/ #
```

Boot reaches a shell prompt with most OpenRC services starting successfully.

### Remaining gaps

| Issue | Root cause |
|-------|-----------|
| `sysctl: fchdir: No such file or directory` | Missing `fchdir(2)` syscall |
| `rm: can't remove '/var/lock': I/O error` | `unlink` on directory needs EISDIR |
| Clock skew warnings | OpenRC re-caches deps when config mtime differs |
| ext4 checksum mismatch after boot | crc32c not computed for superblock/GDT writes |

## Files changed

- `services/kevlar_ext2/src/lib.rs` — cross-dir rename, extent truncation, chown, ordered barriers, dirty flag, timestamps
- `exts/virtio_blk/lib.rs` — VIRTIO_BLK_T_FLUSH implementation
- `kernel/syscalls/mod.rs` — sync(2) calls ext2 flush
