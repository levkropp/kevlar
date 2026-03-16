# Introduction

Kevlar is a Rust kernel for running Linux binaries — it implements the Linux ABI so
that unmodified Linux programs run on Kevlar directly. It is not a Linux fork or a
translation layer; it is a clean-room implementation of the Linux syscall interface
on a new kernel.

Licensed under **MIT OR Apache-2.0 OR BSD-2-Clause**, Kevlar is a clean-room
implementation derived from Linux man pages and POSIX specifications, remaining
fully permissively licensed.

## Current Status

**M10 (Alpine text-mode boot) in progress.** 141 syscall modules, 121+ dispatch
entries. What works today:

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

### Milestones

| Milestone | Status | Description |
|-----------|--------|-------------|
| M1–M6 | **Complete** | Static/dynamic binaries, terminal, job control, epoll, unix sockets, SMP threading, ext2, benchmarks |
| M7: /proc + glibc | **Complete** | Full /proc, glibc compatibility, futex ops |
| M8: cgroups + namespaces | **Complete** | cgroups v2, UTS/mount/PID namespaces, pivot\_root |
| M9: Init system | **Complete** | Syscall gaps, init sequence, OpenRC boots |
| M10: Alpine text-mode | **In Progress** | getty login, ext2 rw, networking, APK |
| M11: Alpine graphical | Planned | Framebuffer, Wayland |

## Architecture

Kevlar uses the **ringkernel** architecture: a single-address-space kernel with
concentric trust zones enforced by Rust's type system, crate visibility, and panic
containment at ring boundaries. See [The Ringkernel Architecture](architecture/ringkernel.md).

## Vision

Kevlar's goal is to become a permissively-licensed drop-in Linux kernel replacement
that runs modern distributions (targeting Kubuntu 24.04) with performance and
security matching or exceeding Linux. It occupies a unique niche: a true Linux-ABI
kernel (not a compatibility shim), built on clean MIT/Apache-2.0/BSD-2-Clause Rust
foundations.

## Links

- [GitHub](https://github.com/levkropp/kevlar)
