# Milestone 5: Persistent Storage & Broader Compatibility

**Goal:** Mount and read a real filesystem from a VirtIO block device. Run
programs loaded from disk. Fill the syscall gaps that real-world software hits.

**Current state:** Kevlar boots from an in-memory initramfs with ~107 syscalls.
All programs must be baked into the initramfs at build time. No disk I/O, no
file change monitoring, many commonly-used file metadata syscalls missing.

## Phases

| Phase | Name | New Syscalls | Prerequisite |
|-------|------|-------------|--------------|
| [1](phase1-file-metadata.md) | File Metadata & Extended Ops | ~8 | None |
| [2](phase2-inotify.md) | inotify | 3 | None |
| [3](phase3-zero-copy-io.md) | Zero-Copy I/O | ~4 | None |
| [4](phase4-proc-sys.md) | /proc & /sys Completeness | 0 (pseudo-fs) | None |
| [5](phase5-virtio-block.md) | VirtIO Block Driver | 0 (driver) | None |
| [6](phase6-ext2.md) | Read-Only ext2 Filesystem | 0 (filesystem) | Phase 5 |
| [7](phase7-integration.md) | Integration Testing | stubs + fixes | All above |

Phases 1-4 are independent syscall/pseudo-fs work (can be done in any order).
Phase 5 is the VirtIO block driver (hardware layer).
Phase 6 builds on Phase 5 to provide a real filesystem.
Phase 7 is the final integration pass.

## Why This Milestone

Without persistent storage, Kevlar is limited to whatever fits in the
initramfs. This ceiling blocks every future milestone:

- Package managers need to write files to disk
- Real distro images live on block devices
- Larger programs (compilers, interpreters) don't fit in initramfs easily
- Development workflows need persistent state

Phases 1-4 also close the most common syscall gaps that real programs hit —
`statfs`, `inotify`, `sendfile`, `/proc/self/maps` — making the kernel
useful for a much broader class of static musl binaries.

## Architectural Constraints

- All new subsystems must respect the ringkernel architecture:
  - VirtIO block driver goes in Platform/Ring 0 (hardware interaction)
  - ext2 filesystem goes in Services/Ring 2 (safe Rust, panic-contained)
  - New /proc files go in kernel core (Ring 1)
  - No new `unsafe` in the kernel crate
- ext2 implementation is **clean-room from the specification**:
  - Reference: FreeBSD `sys/fs/ext2fs/` (BSD-2-Clause)
  - Reference: OSDev wiki ext2 documentation
  - Reference: "The Second Extended Filesystem" by Dave Poirier
  - Do NOT reference Linux `fs/ext2/` (GPL)
- VirtIO block driver references:
  - VirtIO specification v1.2 (OASIS standard, freely available)
  - Existing VirtIO-net driver in Kevlar (same queue infrastructure)
  - FreeBSD `sys/dev/virtio/block/` (BSD-2-Clause)
- Write all new code with SMP in mind (proper locking, atomics) even though
  M6 adds actual multi-CPU support. This avoids a codebase-wide audit later.

## Key Design Decisions

1. **ext2 read-only first.** Write support requires block allocation, inode
   updates, directory entry insertion, and crash consistency. Read-only ext2
   is ~1/3 the complexity and sufficient for loading programs from disk.
   Write support deferred to M7.

2. **VirtIO PCI, not legacy.** Modern VirtIO uses PCI capabilities for device
   discovery. We already have VirtIO-net on PCI. ARM64 uses VirtIO-MMIO
   (already have the transport for virtio-net).

3. **Block cache.** Start with a simple LRU or direct-mapped block cache.
   The page cache from the performance work (Phase D) may be adaptable.
   ext2 reads many small metadata blocks, so caching matters.

4. **inotify scope.** Start with tmpfs/initramfs support only. Extending to
   ext2 can come later (requires hooking into the VFS write path, which we
   don't have for ext2 yet since it's read-only).

## Success Criteria

- `statfs /` returns sensible values for tmpfs
- `inotify` works with epoll for file change detection
- `sendfile` transfers data between fds efficiently
- `/proc/self/maps` shows the process memory layout
- VirtIO block device detected and reads sectors correctly
- ext2 filesystem mounts read-only, files readable
- A test binary loaded from ext2 disk image executes successfully
- All existing tests (bench, mini_systemd) still pass
