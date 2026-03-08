# Reviving Kerla: Forking a Dead Rust Kernel

*Date: 2026-03-08*

## Why Kevlar?

We wanted a permissively licensed, Rust-based, Linux-compatible kernel capable of running
real Linux userspace software. The landscape of Rust OS kernels in 2026 looks like this:

- **Asterinas** (MPL-2.0) - The most mature option with 170+ syscalls,
  but its copyleft license doesn't fit our use case.
- **Kerla** (MIT/Apache-2.0) - A clean ~15k-line kernel targeting Linux binary compatibility,
  but abandoned since late 2021 with only 59 syscalls.
- **Redox** (MIT) - A microkernel with its own userspace, not targeting Linux ABI compatibility.

We chose to fork Kerla as our foundation. Despite being unmaintained, it provides:
- The right license (MIT OR Apache-2.0)
- A working bootloader and basic kernel infrastructure
- 59 implemented syscalls covering the basics (fork, execve, read, write, mmap, networking)
- A clean, readable codebase (~15k lines of Rust)

We also identified **OSv** (BSD-3-Clause), a C/C++ unikernel with proven VFS, epoll, ext2,
and mmap implementations that we can port to Rust with attribution.

## What We Did

Phase 0 consisted of:

1. **Fork and rename**: All references to `kerla` changed to `kevlar` across 100+ files
2. **License setup**: SPDX headers on every `.rs` file, NOTICE file for attribution
3. **Documentation**: mdBook with architecture docs, provenance tracking, clean-room log
4. **Toolchain update**: Migrating from Rust nightly-2024-01-01 to latest nightly

## What's Next

Phase 0.5 will establish a framekernel architecture (inspired by studying Asterinas's design)
where all `unsafe` code is confined to a HAL crate and the kernel uses `#![deny(unsafe_code)]`.

Then the real work begins: threading (clone/futex), memory management (mprotect/munmap/COW),
signals (SA_SIGINFO/sigaltstack), and all the other subsystems needed to run real Linux software.

## Clean-Room Discipline

We maintain a strict provenance log. Asterinas is studied for design patterns only -
no code is copied. OSv code is ported to Rust with BSD-3-Clause attribution retained.
All of this is documented in our [clean-room log](../Documentation/provenance/clean-room-log.md).
