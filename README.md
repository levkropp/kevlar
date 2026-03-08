# Kevlar

A permissively licensed Rust kernel for running Linux binaries, informed by FreeBSD and OSv.

## Overview

Kevlar is a monolithic operating system kernel written in Rust that aims for Linux ABI
binary compatibility. It is a fork of [Kerla](https://github.com/nuta/kerla) (MIT/Apache-2.0),
modernized, extended, and relicensed as MIT/Apache-2.0/BSD-2-Clause with the goal of becoming
the most capable permissively licensed Rust kernel for running real Linux userspace software.

**License:** MIT OR Apache-2.0 OR BSD-2-Clause

## Current Status

After completing M2, the kernel boots static musl BusyBox, runs an interactive shell on both x86_64 and ARM64, and supports dynamically linked musl PIE binaries with the musl dynamic linker.

- **83 implemented syscalls** (90 dispatch entries)
- **Dynamic linking:** PIE executables with PT_INTERP, dual ELF loading, auxiliary vectors
- **Two architectures:** x86_64 and ARM64 (QEMU virt, cortex-a72)
- Full memory management: mmap (MAP_FIXED), mprotect, munmap, madvise with NX bit support
- File operations: openat, newfstatat, lseek, pread64, unlink, rmdir, rename
- Process control: fork, vfork, execve, wait4 (correct status encoding), signals, futex
- FD plumbing: dup, dup2, dup3, pipe, pipe2, fcntl (F_GETFD/F_SETFD/F_GETFL/F_SETFL)
- Networking: TCP/UDP via smoltcp (virtio-net), socket, bind, listen, accept, connect
- Time: clock_gettime, gettimeofday, nanosleep
- System info: uname, sysinfo, getrlimit, syslog
- Initramfs root filesystem (tmpfs, basic procfs, devfs)
- TTY and pseudo-terminal (PTY) support
- QEMU and Firecracker support (x86_64); QEMU virt (ARM64)

## Goals

1. **Full Linux ABI compatibility** — Run real Linux userspace binaries unmodified
2. **Permissive licensing** — All code is MIT/Apache-2.0/BSD-2-Clause or compatible
3. **Clean-room provenance** — Syscall semantics informed by FreeBSD's linuxulator (BSD-2-Clause); VFS abstractions adapted from OSv (BSD-3-Clause); design inspired by Asterinas (studied, not copied)
4. **170+ syscalls** — Full coverage for threading, signals, memory management, filesystems, and networking

## Roadmap

| Milestone | Syscalls | Status | Description |
|-----------|----------|--------|-------------|
| M1: Static Busybox | ~50 | **Complete** | Core syscalls for static musl binaries |
| M1.5: ARM64 | -- | **Complete** | ARM64 port; BusyBox boots on QEMU virt (cortex-a72) |
| M2: Dynamic linking | ~55 | **Complete** | PIE + musl ld-linux; pread64, futex, madvise |
| M3: Coreutils + Bash | ~80 | Planned | clone, job control, symlinks |
| M4: systemd | ~110 | Planned | epoll, signalfd, timerfd, mount |
| M5: apt/dpkg | ~120 | Planned | xattr, statx, splice |
| M6: Full networking | ~130 | Planned | AF_NETLINK, accept4 |
| M7: Container runtime | ~145 | Planned | namespaces, seccomp, clone3 |
| M8: Kubuntu 24.04 | ~170 | Planned | SysV IPC, ptrace, io_uring |

See [Documentation/compatibility.md](Documentation/compatibility.md) for the full syscall-by-syscall status table.

## Provenance & Attribution

Kevlar is built from permissively licensed sources:

| Source | License | Usage |
|--------|---------|-------|
| [Kerla](https://github.com/nuta/kerla) | MIT OR Apache-2.0 | Fork base (original kernel) |
| [FreeBSD](https://github.com/freebsd/freebsd-src) | BSD-2-Clause | Primary reference for Linux syscall semantics (linuxulator: sys/compat/linux/) |
| [OSv](https://github.com/cloudius-systems/osv) | BSD-3-Clause | Reference for VFS layer, filesystem abstractions |
| [Asterinas](https://github.com/asterinas/asterinas) | MPL-2.0 | Design reference only (no code) |

See [NOTICE](NOTICE) for full attribution details.

## Building

Kevlar uses Rust nightly. To build:

```bash
# Install Rust nightly
rustup install nightly

# Build and run on x86_64
make run

# Build and run on ARM64 (release required for TCG performance)
RELEASE=1 ARCH=arm64 make run
```

## Documentation

- [Documentation/](Documentation/) — The Kevlar Book (architecture, syscall coverage, provenance log)
- [blog/](blog/) — Development milestone blog posts
- [M1-PLAN.md](M1-PLAN.md) — Static Busybox implementation plan and status

## License

Licensed under any of:

- MIT license ([LICENSE.md](LICENSE.md))
- Apache License, Version 2.0 ([LICENSE.md](LICENSE.md))
- BSD-2-Clause license ([LICENSE.md](LICENSE.md))

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion
in this project shall be tri-licensed as above, without any additional terms or conditions.
