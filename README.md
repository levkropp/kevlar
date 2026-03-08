# Kevlar

A permissively licensed, Rust-based, Linux-compatible kernel.

## Overview

Kevlar is a monolithic operating system kernel written in Rust that aims for Linux ABI
binary compatibility. It is a fork of [Kerla](https://github.com/nuta/kerla) (MIT/Apache-2.0),
modernized and extended with the goal of becoming the most capable permissively licensed
Rust kernel for running real Linux userspace software.

**License:** Dual MIT OR Apache-2.0

## Current Status

Kevlar is in early development (Phase 0). The kernel currently supports:

- 59 Linux syscalls (fork, execve, read, write, mmap, pipe, poll, select, signals, etc.)
- x86_64 architecture
- smoltcp-based TCP/IP networking (virtio-net)
- Initramfs root filesystem with tmpfs, basic procfs, and devfs
- TTY and pseudo-terminal (PTY) support
- QEMU and Firecracker support

## Goals

1. **Full Linux ABI compatibility** - Run real Linux userspace binaries unmodified
2. **Permissive licensing** - All code is MIT/Apache-2.0 or BSD-3-Clause compatible
3. **Clean-room design** - Architecture inspired by Asterinas (studied, not copied), code ported from OSv (BSD-3-Clause) where applicable
4. **170+ syscalls** - Full coverage for threading, signals, memory management, filesystems, and networking

## Roadmap

| Phase | Status | Description |
|-------|--------|-------------|
| 0: Fork & Modernize | In Progress | Rename, update toolchain, CI, documentation |
| 0.5: HAL/Kernel Split | Planned | Framekernel architecture, `#![deny(unsafe_code)]` in kernel |
| 1: Threading | Planned | clone(), futex, robust lists |
| 2: Memory Management | Planned | mprotect, munmap, mremap, COW, VMAR/VMO |
| 3: Signals | Planned | SA_SIGINFO, sigaltstack, signal queues, ptrace |
| 4: Events | Planned | epoll, eventfd, timerfd, signalfd |
| 5: Filesystem | Planned | Comprehensive procfs, ext2, sysfs |
| 6: Networking | Planned | Unix domain sockets, SCM_RIGHTS |
| 7: Real Workloads | Planned | Boot a full Linux userspace, run complex applications |

## Provenance & Attribution

Kevlar is built from permissively licensed sources:

| Source | License | Usage |
|--------|---------|-------|
| [Kerla](https://github.com/nuta/kerla) | MIT OR Apache-2.0 | Fork base (original kernel) |
| [OSv](https://github.com/cloudius-systems/osv) | BSD-3-Clause | Port C/C++ subsystems to Rust |
| [Asterinas](https://github.com/asterinas/asterinas) | MPL-2.0 | Design reference only (no code) |

See [NOTICE](NOTICE) for full attribution details.

## Building

Kevlar uses Rust nightly. To build:

```bash
# Install Rust nightly
rustup install nightly

# Build the kernel
make build

# Run in QEMU
make run
```

## Documentation

- `book/` - The Kevlar Book (architecture, syscall coverage, provenance log)
- `blog/` - Development milestone blog posts

## License

Licensed under either of:

- MIT license ([LICENSE.md](LICENSE.md))
- Apache License, Version 2.0 ([LICENSE.md](LICENSE.md))

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion
in this project shall be dual licensed as above, without any additional terms or conditions.
