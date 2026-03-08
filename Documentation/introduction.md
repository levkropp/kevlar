# Introduction

Kevlar is a monolithic operating system kernel written in Rust which aims to be compatible
with the Linux ABI — it runs Linux binaries without any modifications.

Licensed under MIT OR Apache-2.0, Kevlar is strongly informed by FreeBSD's linuxulator
(BSD-2-Clause) for Linux syscall semantics and OSv (BSD-3-Clause) for VFS abstractions.
This gives Kevlar a provably clean-room path to Linux binary compatibility while remaining
fully permissively licensed.

## Current Status

**Milestone 1 complete:** Static musl BusyBox boots on Kevlar in QEMU and runs an
interactive shell. 79 syscalls are implemented across process management, file I/O,
memory management, networking (TCP/UDP via smoltcp), signals, and TTY support.

## Vision

Kevlar occupies a unique position in the OS landscape: more Linux-native than FreeBSD's
compatibility layer (Kevlar IS a Linux-ABI kernel, not a translation layer), but built
on clean BSD/MIT-licensed Rust foundations. The goal is a kernel that Linux's ecosystem
deserves — permissively licensed, memory-safe, with the correctness guarantees that come
from FreeBSD's decades of POSIX expertise.

## Links

- [GitHub](https://github.com/levkropp/kevlar)
