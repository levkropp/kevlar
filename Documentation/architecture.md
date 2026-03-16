# Architecture Overview

Kevlar is organized as a **ringkernel**: a single-address-space kernel with three
concentric trust zones enforced by Rust's type system and crate visibility. For the
full architectural design, see [The Ringkernel Architecture](architecture/ringkernel.md).

## Crate Layout

```
kevlar/
├── kernel/          # Ring 1: Core OS logic (safe Rust, #![deny(unsafe_code)])
│   ├── process/     # Process lifecycle, scheduler, signals
│   ├── mm/          # Virtual memory, demand paging, page fault handler
│   ├── fs/          # VFS dispatch, procfs, sysfs, devfs, inotify, epoll
│   ├── net/         # smoltcp integration, TCP/UDP/ICMP/Unix sockets
│   ├── syscalls/    # Syscall dispatch and implementations
│   ├── cgroups/     # cgroups v2 hierarchy and pids controller
│   └── namespace/   # UTS, PID, and mount namespaces
├── platform/        # Ring 0: Hardware interface (unsafe Rust, minimal TCB)
│   ├── x64/         # x86_64: APIC, paging, SMP, vDSO, TSC, usercopy
│   └── arm64/       # ARM64: GIC, PSCI, generic timer
├── libs/
│   └── kevlar_vfs/  # Shared VFS types (#![forbid(unsafe_code)])
├── services/
│   ├── kevlar_ext2/        # ext2/3/4 read-write filesystem (#![forbid(unsafe_code)])
│   ├── kevlar_tmpfs/       # tmpfs (#![forbid(unsafe_code)])
│   └── kevlar_initramfs/   # initramfs cpio parser (#![forbid(unsafe_code)])
└── exts/
    └── virtio_net/  # VirtIO network driver
```

## Core Abstractions

### INode

`INode` is an enum representing any filesystem object:

```rust
pub enum INode {
    FileLike(Arc<dyn FileLike>),
    Directory(Arc<dyn Directory>),
    Symlink(Arc<dyn Symlink>),
}
```

All filesystem operations go through the `FileLike`, `Directory`, or `Symlink` traits.
The kernel holds `INode` values and never calls filesystem-specific code directly.

### FileLike

`FileLike` is the trait for file-like I/O. It covers `read`, `write`, `ioctl`, `poll`,
`mmap`, `stat`, `truncate`, `fsync`, and socket operations. Sockets, pipes, TTY devices,
regular files, epoll instances, signalfd, timerfd, and eventfd all implement it.

### VFS and Path Resolution

Paths are resolved through a tree of `PathComponent` nodes, one per path segment. The
mount table intercepts lookups at mount points using `MountKey` (dev\_id + inode\_no)
for collision-free matching across filesystems.

Path resolution has two paths:
- **Fast path** — direct directory tree walk (no `..`, no symlinks in intermediates)
- **Full path** — builds a `PathComponent` chain, follows symlinks (up to 8 hops), resolves `..`

### Process

A `Process` holds:
- Platform execution context (saved registers, kernel stack, xsave FPU state)
- Virtual memory map (`Vm`) — VMA list + page table (shared across threads via `Arc`)
- Open file table (`OpenedFileTable`) — fd to `Arc<dyn FileLike>` (shared across threads)
- Signal state — `SignalDelivery` (handlers, pending) + `AtomicU64` mask (lock-free)
- Thread group ID (`tgid`) for POSIX thread semantics
- cgroup membership and namespace set
- Process group and session for job control

`Arc<SpinLock<...>>` on `vm` and `opened_files` supports `clone(CLONE_VM | CLONE_FILES)`
as used by `pthread_create`.

See [Process & Thread Model](architecture/process.md) for details.

### WaitQueue

A `WaitQueue` holds a list of blocked processes waiting for an event (e.g., a child
exiting, new data on a socket). `sleep_signalable_until` blocks the caller until a
predicate returns `Some`, and is woken by `wake_all` / `wake_one`.

## Key Design Properties

| Property | Value |
|---|---|
| Address spaces | Single (kernel + user in one virtual space) |
| Unsafe code | Confined to `platform/` crate only |
| SMP | Per-CPU run queues with work stealing (up to 8 CPUs) |
| Panic behavior | Ring 2 panics caught → return `EIO`; kernel continues |
| IPC overhead | None — all ring crossings are direct function calls |
| Page sharing | Copy-on-write via per-page refcounting |
| Huge pages | Transparent 2 MB pages for anonymous mappings |
| License | MIT OR Apache-2.0 OR BSD-2-Clause |

## Subsystem Pages

- [The Ringkernel Architecture](architecture/ringkernel.md) — trust rings, safety design
- [Safety Profiles](architecture/safety-profiles.md) — Fortress / Balanced / Performance / Ludicrous
- [Platform / HAL](architecture/hal.md) — Ring 0, hardware abstraction, SMP
- [Memory Management](architecture/memory.md) — VMAs, demand paging, CoW, huge pages
- [Process & Thread Model](architecture/process.md) — lifecycle, SMP scheduler, threads, cgroups, namespaces
- [Signal Handling](architecture/signals.md) — POSIX signals, delivery, masking, signalfd
- [Filesystems](architecture/filesystems.md) — VFS, initramfs, tmpfs, ext2, procfs, sysfs, devfs
- [Networking](architecture/networking.md) — smoltcp, Unix sockets, ICMP, epoll
