# Introduction

Kevlar is a Rust kernel for running Linux binaries — it implements the Linux ABI so
that unmodified Linux programs run on Kevlar directly. It is not a Linux fork or a
translation layer; it is a clean-room implementation of the Linux syscall interface
on a new kernel.

Licensed under **MIT OR Apache-2.0 OR BSD-2-Clause**, Kevlar draws on FreeBSD's
linuxulator (BSD-2-Clause) as the primary reference for Linux syscall semantics.
This gives Kevlar a provably clean-room path to Linux ABI compatibility while
remaining fully permissively licensed.

## Current Status

**M5 Phase 4 complete.** 107+ syscalls implemented. The following work:

- Static and dynamic (PIE) musl-linked binaries
- BusyBox interactive shell on x86\_64 and ARM64
- TCP/UDP networking via virtio-net (smoltcp 0.12)
- Unix domain sockets with SCM\_RIGHTS
- Full POSIX signal semantics (SA\_SIGINFO, sigaltstack, sigprocmask)
- Job control and terminal management (SIGTSTP, SIGCONT, tcsetpgrp)
- epoll, eventfd, inotify, timerfd, signalfd
- Mount namespace (bind mounts, pivot\_root)
- procfs and devfs (including /proc/[pid]/maps, /proc/[pid]/fd/, /proc/cpuinfo)
- Process capabilities and prctl
- vDSO for fast clock\_gettime (10 ns, 2× faster than Linux KVM)
- KVM-accelerated performance matching or exceeding Linux for many syscalls

## Architecture

Kevlar uses the **ringkernel** architecture: a single-address-space kernel with
concentric trust zones enforced by Rust's type system, crate visibility, and panic
containment at ring boundaries. See [The Ringkernel Architecture](architecture/ringkernel.md).

## Vision

Kevlar's goal is to run the Linux ecosystem — including Wine and complex desktop
applications — on a permissively licensed, memory-safe kernel. It occupies a unique
niche: more Linux-native than FreeBSD's linuxulator (Kevlar *is* a Linux-ABI kernel,
not a compatibility shim), but built on clean BSD/MIT-licensed Rust foundations.

## Links

- [GitHub](https://github.com/levkropp/kevlar)
