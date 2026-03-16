# Architecture Overview

Kevlar is organized as a **ringkernel**: a single-address-space kernel with three
concentric trust zones enforced by Rust's type system and crate visibility. For the
full architectural design, see [The Ringkernel Architecture](architecture/ringkernel.md).

## Crate Layout

```
kevlar/
├── kernel/          # Ring 1: Core OS logic (safe Rust, #![deny(unsafe_code)])
├── platform/        # Ring 0: Hardware interface (unsafe Rust, minimal TCB)
├── libs/
│   └── kevlar_vfs/  # Shared VFS types (#![forbid(unsafe_code)])
└── services/
    ├── kevlar_ext2/        # ext2 read-write filesystem (#![forbid(unsafe_code)])
    ├── kevlar_tmpfs/       # tmpfs (#![forbid(unsafe_code)])
    └── kevlar_initramfs/   # initramfs parser (#![forbid(unsafe_code)])
```

## Core Abstractions

### INode

`INode` is an enum representing any filesystem object:

- **`FileLike`** — regular files, pipes, sockets, `/dev/null`, eventfd, epoll, etc.
- **`Directory`** — directory objects; supports `lookup` and `readdir`
- **`Symlink`** — symbolic links; returns a target path

### FileLike

`FileLike` is the trait for file-like I/O. It covers `read`, `write`, `ioctl`, `poll`,
`mmap`, and `stat`. Sockets, pipes, TTY devices, and regular files all implement it.

### VFS and Path Resolution

Paths are resolved through a tree of `PathComponent` nodes, one per path segment.
The mount table intercepts lookups at mount points. The root filesystem is initramfs
at boot; additional mounts layer on top via `mount(2)`.

### Process

A `Process` holds:
- A kernel task (saved registers, kernel stack, FPU state via xsave)
- A virtual memory map (`Vm`) — the list of VMAs and the page table
- An open file table (`OpenedFileTable`) — fd → `Arc<dyn FileLike>`
- Signal state (`SignalDelivery`) — handlers, mask, pending set
- Process group and session for job control

See [Process & Thread Model](architecture/process.md) for details.

### WaitQueue

A `WaitQueue` holds a list of blocked processes waiting for an event (e.g., a child
exiting, new data on a socket). `wait_event` sleeps until a `wake_all` call resumes
all waiters.

## Key Design Properties

| Property | Value |
|---|---|
| Address spaces | Single (kernel + user in one virtual space) |
| Unsafe code | Confined to `platform/` crate only |
| Panic behavior | Ring 2 panics caught → return `EIO`; kernel continues |
| IPC overhead | None — all ring crossings are direct function calls |
| License | MIT OR Apache-2.0 OR BSD-2-Clause |

## Subsystem Pages

- [The Ringkernel Architecture](architecture/ringkernel.md) — trust rings, safety design
- [Safety Profiles](architecture/safety-profiles.md) — Fortress / Balanced / Performance / Ludicrous
- [Platform / HAL](architecture/hal.md) — Ring 0, hardware abstraction
- [Memory Management](architecture/memory.md) — VMAs, demand paging, page cache
- [Process & Thread Model](architecture/process.md) — process lifecycle, scheduling
- [Signal Handling](architecture/signals.md) — POSIX signals, delivery, masking
- [Filesystems](architecture/filesystems.md) — VFS, initramfs, tmpfs, procfs, devfs
- [Networking](architecture/networking.md) — smoltcp, Unix sockets, virtio-net
