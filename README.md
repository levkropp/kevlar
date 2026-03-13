# Kevlar

A permissively licensed Rust kernel for running Linux binaries.

## Overview

Kevlar implements the Linux ABI so that unmodified Linux programs run directly
on it — not through a compatibility shim, but because Kevlar *is* a Linux-ABI
kernel. It is a fork of [Kerla](https://github.com/nuta/kerla) (MIT/Apache-2.0),
modernized, extended, and relicensed as MIT/Apache-2.0/BSD-2-Clause.

Kevlar uses the **ringkernel** architecture: a single-address-space kernel with
three concentric trust zones enforced by Rust's type system. Unsafe code is
confined to the `platform/` crate (<10% of the codebase). All syscall handlers,
VFS logic, process management, and networking are written in safe Rust.

**License:** MIT OR Apache-2.0 OR BSD-2-Clause

## Current Status

**M5 Phase 4 complete.** 107+ syscalls implemented.

- Static and dynamic (PIE) musl-linked binaries
- BusyBox interactive shell on x86\_64 and ARM64 (QEMU)
- TCP/UDP networking via virtio-net (smoltcp 0.12)
- Unix domain sockets with SCM\_RIGHTS
- Full POSIX signal semantics (SA\_SIGINFO, sigaltstack, lock-free sigprocmask)
- Job control and terminal management (SIGTSTP, SIGCONT, tcsetpgrp)
- epoll, eventfd, inotify, timerfd, signalfd
- Mount namespace support (bind mounts)
- procfs and devfs (/proc/[pid]/maps, /proc/[pid]/fd/, /proc/cpuinfo)
- Process capabilities and prctl
- vDSO for fast clock\_gettime (~10 ns; 2× faster than Linux KVM)
- KVM performance at or above Linux for many syscalls (getpid: 200 ns)

## Roadmap

| Milestone | Syscalls | Status | Description |
|-----------|----------|--------|-------------|
| M1: Static Busybox | ~50 | **Complete** | Core syscalls for static musl binaries |
| M1.5: ARM64 | -- | **Complete** | ARM64 port; BusyBox boots on QEMU virt |
| M2: Dynamic linking | ~55 | **Complete** | PIE + musl ld-linux; pread64, futex, madvise |
| M3: Terminal + Job Control | ~80 | **Complete** | Terminal, job control, symlinks, clone |
| M4: systemd-compatible PID 1 | ~110 | **Complete** | epoll, unix sockets, mount, caps; 15/15 integration tests |
| M5: Persistent Storage | ~120 | In Progress | statx, inotify, procfs done; VirtIO block + ext2 next |
| M6: Full networking | ~130 | Planned | AF\_NETLINK, accept4 |
| M7: Container runtime | ~145 | Planned | namespaces, seccomp, clone3 |
| M8: Kubuntu 24.04 desktop | ~170 | Planned | SysV IPC, ptrace, io\_uring |

See [Documentation/compatibility.md](Documentation/compatibility.md) for the
full syscall-by-syscall status.

## Goals

1. **Full Linux ABI compatibility** — Run real Linux userspace binaries unmodified
2. **Permissive licensing** — All code is MIT/Apache-2.0/BSD-2-Clause or compatible
3. **Ringkernel architecture** — Three-ring safety design: unsafe Platform (<10% TCB),
   safe Core, panic-contained Services. Near-monolithic performance with
   microkernel-style fault isolation via `catch_unwind` at ring boundaries
4. **Configurable safety** — Four compile-time profiles from Fortress (maximum safety)
   to Ludicrous (maximum performance)
5. **Clean-room provenance** — Syscall semantics derived from FreeBSD's linuxulator
   (BSD-2-Clause); no GPL code ever copied

## Building

```bash
rustup install nightly
rustup override set nightly
rustup component add llvm-tools-preview rust-src

make run               # Build and boot on x86_64
make ARCH=arm64 RELEASE=1 run  # ARM64 (release for TCG performance)
```

**Windows users**: The Makefile automatically uses WSL2. Just run `make run` - it works!

See [Documentation/quickstart.md](Documentation/quickstart.md) for full build
instructions including Docker, prerequisites, and make targets.

## Provenance & Attribution

| Source | License | Usage |
|--------|---------|-------|
| [Kerla](https://github.com/nuta/kerla) | MIT OR Apache-2.0 | Fork base |
| [FreeBSD](https://github.com/freebsd/freebsd-src) | BSD-2-Clause | Primary reference for Linux syscall semantics |

See [Documentation/provenance/](Documentation/provenance/) for the full clean-room
implementation log and attribution details.

## Documentation

- [Documentation/](Documentation/) — Architecture, syscall coverage, provenance log
- [Documentation/blog/](Documentation/blog/) — Development milestone blog posts

## License

Licensed under any of:

- MIT license ([LICENSE.md](LICENSE.md))
- Apache License, Version 2.0 ([LICENSE.md](LICENSE.md))
- BSD-2-Clause license ([LICENSE.md](LICENSE.md))

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project shall be tri-licensed as above, without any
additional terms or conditions.
