# Kevlar

A permissively licensed Rust kernel that runs unmodified Linux binaries.

## Overview

Kevlar implements the Linux ABI so that real Linux programs — glibc, musl,
BusyBox, Alpine's APK, OpenRC — run on it directly. It is not a compatibility
shim; it *is* a Linux-ABI kernel written from scratch in safe Rust.

**License:** MIT OR Apache-2.0 OR BSD-2-Clause

## Current Status

**M10 (Alpine text-mode boot) in progress.** 141 syscall modules, 121+
dispatch entries.

What works today:

- glibc and musl dynamically-linked binaries (PIE)
- BusyBox interactive shell on x86\_64 and ARM64
- Alpine Linux boots with OpenRC init and getty login
- ext2 read-write filesystem on VirtIO block
- TCP/UDP/ICMP networking via virtio-net (smoltcp 0.12)
- Unix domain sockets with SCM\_RIGHTS
- SMP: per-CPU scheduling, work stealing, TLB shootdown, clone threads
- Full POSIX signals (SA\_SIGINFO, sigaltstack, lock-free sigprocmask)
- epoll, eventfd, inotify, timerfd, signalfd
- cgroups v2 (pids controller), UTS/mount/PID namespaces
- procfs, sysfs, devfs
- vDSO clock\_gettime (~10 ns, 2x faster than Linux KVM)
- 4 compile-time safety profiles (Fortress to Ludicrous)

## Roadmap

| Milestone | Status | Description |
|-----------|--------|-------------|
| M1-M6 | **Complete** | Static/dynamic binaries, terminal, job control, epoll, unix sockets, SMP threading, ext2, benchmarks |
| M7: /proc + glibc | **Complete** | Full /proc, glibc compatibility, futex ops |
| M8: cgroups + namespaces | **Complete** | cgroups v2, UTS/mount/PID namespaces, pivot\_root |
| M9: Init system | **Complete** | Syscall gaps, init sequence, OpenRC boots |
| M10: Alpine text-mode | **In Progress** | getty login, ext2 rw, networking, APK |
| M11: Alpine graphical | Planned | Framebuffer, Wayland |

**Ultimate goal:** Run Kubuntu 24.04 with full desktop, matching or exceeding
Linux performance.

## Architecture

Kevlar uses the **ringkernel** design — three concentric trust zones in a
single address space, enforced by Rust's type system:

- **Platform** (Ring 0) — all unsafe code, <10% of codebase
- **Core** (Ring 1) — safe Rust, OS policy (`#![deny(unsafe_code)]`)
- **Services** (Ring 2) — safe Rust, panic-contained via `catch_unwind`
  (`#![forbid(unsafe_code)]`)

Near-monolithic performance with microkernel-style fault isolation.

## Building

```bash
rustup install nightly
rustup override set nightly
rustup component add llvm-tools-preview rust-src

make run                       # x86_64 in QEMU
make ARCH=arm64 RELEASE=1 run  # ARM64
make bench-kvm                 # KVM benchmarks
```

**Windows:** The Makefile uses WSL2 automatically. See [Documentation/building.md](Documentation/building.md) for WSL setup instructions.

## Provenance

Originally forked from [Kerla](https://github.com/nuta/kerla) (MIT OR
Apache-2.0) by Seiya Nuta. Substantially rewritten — see
[Documentation/provenance/](Documentation/provenance/) for the full
attribution and clean-room log.

## License

Licensed under any of:

- MIT license
- Apache License, Version 2.0
- BSD-2-Clause license

at your option. See [LICENSE.md](LICENSE.md).

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project shall be tri-licensed as above, without any
additional terms or conditions.
