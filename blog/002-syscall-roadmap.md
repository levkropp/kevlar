# The Road to 170 Syscalls: A Milestone-Driven Approach

*Date: 2026-03-08*

## Where We Stand

Kevlar inherits 59 syscalls from Kerla. After auditing every implementation against the
actual source code, we found that roughly 35 are meaningfully functional — the rest are
stubs that return 0 (like `getuid`, `setgid`, `setgroups`) or have significant gaps
(like `mmap` ignoring protection flags entirely).

The question is: what do we implement next, and in what order?

Rather than working down the syscall table numerically, we defined eight compatibility
milestones — each one a real workload that we can test against. Each milestone builds on
the previous one, and the syscalls we need to add form a natural dependency graph.

## The Eight Milestones

### M1: Static Busybox (~50 syscalls, ~15 to add)

The first real test: run a statically-linked BusyBox binary. This gives us `sh`, `ls`,
`cat`, `grep`, `mv`, `rm`, and dozens of other utilities in a single binary.

Kevlar already has most of what BusyBox needs — basic file I/O, fork/exec, pipes, signals,
and sockets. The critical gaps are:

- **`lseek`(8)** — can't `cat` a file without seeking
- **`mprotect`(10), `munmap`(11)** — even static musl sets up stack guard pages
- **`openat`(257), `newfstatat`(262)** — modern musl uses `*at` variants exclusively, not `open`/`stat`
- **`dup`(32), `pipe2`(293), `dup3`(292)** — shell plumbing
- **`unlink`(87), `rmdir`(84), `rename`(82)** — can't `rm` or `mv` files
- **`nanosleep`(35), `gettimeofday`(96)** — the `sleep` and `date` commands
- **`access`(21), `umask`(95)** — file existence checks, creation mask

Most of these are straightforward. OSv (BSD-3-Clause) has good reference implementations
for `lseek`, `nanosleep`, `access`, `umask`, `rename`, `unlink`, and `rmdir` in its VFS
layer (`fs/vfs/vfs_syscalls.cc`). The memory management calls (`mprotect`, `munmap`) can
reference OSv's `core/mmu.cc` for VMA tracking logic, though we'll need our own page table
integration.

### M2: Dynamic Linking (~55 syscalls, ~5 more to add)

Getting `ld-linux.so` working unlocks every dynamically-linked binary. The dynamic linker's
core loop is: `openat` → `pread64` → `mmap(PROT_READ)` → `mmap(MAP_FIXED, PROT_READ|PROT_EXEC)`
→ `mprotect` → `close`. Without working memory protections, nothing dynamically-linked can run.

New requirements beyond M1:
- **`pread64`(17)** — read ELF segments at specific offsets without seeking
- **`mremap`(25)** — glibc's malloc uses this for large realloc
- **`madvise`(28)** — `MADV_DONTNEED` is used by every allocator to return pages
- **`futex`(202)** — the universal synchronization primitive; needed as soon as pthreads initializes
- **`set_robust_list`(273)** — registered by pthreads at startup
- **`prlimit64`(302)** — query stack size limits
- **`rseq`(334)** — glibc 2.35+ calls this at startup; can safely return `ENOSYS` initially

The `futex` implementation is critical and there's no good permissive-licensed reference. OSv's
futex is too minimal (only `FUTEX_WAIT` and `FUTEX_WAKE`). We need at least `FUTEX_WAIT`,
`FUTEX_WAKE`, `FUTEX_WAIT_BITSET`, `FUTEX_WAKE_BITSET`, and `FUTEX_REQUEUE` for glibc and musl
compatibility. This will be implemented from scratch using the Linux kernel documentation.

### M3: GNU Coreutils + Bash (~80 syscalls, ~25 more)

Bash needs proper job control, which means sessions and process groups:
- **`clone`(56)** — thread and process creation with flags
- **`setsid`(112)** — create new session for job control
- **`sigaltstack`(131)** — alternate signal stack
- **`rt_sigsuspend`(130)** — wait for signal delivery
- **`tgkill`(234)** — send signal to specific thread
- **`waitid`(247)** — extended wait (coreutils uses this)

Filesystem operations get more demanding:
- **`ftruncate`(77)** — shell `>` redirect to existing file
- **`fchdir`(81), `fchmod`(91), `fchown`(93)** — operate on file descriptors
- **`symlink`(88)** / **`symlinkat`(266)** — symbolic links
- **`unlinkat`(263), `renameat2`(316)** — modern `*at` variants
- **`statfs`(137)** — the `df` command
- **`flock`(73)** — advisory file locking in shell scripts

Most of the filesystem `*at` variants can reference OSv's VFS layer. The `clone` syscall
has no suitable permissive reference — OSv's clone only creates threads (no `CLONE_NEWPID`,
no fork semantics). This is one of the most complex syscalls we'll write ourselves.

### M4: systemd (~110 syscalls, ~30 more)

systemd is the gatekeeper to every modern Linux distribution. It's the single most demanding
program in terms of syscall requirements. Its core event loop is built entirely on:

- **`epoll_create1`(291), `epoll_ctl`(233), `epoll_wait`(232)** — the I/O multiplexing backbone
- **`signalfd4`(289)** — receive signals as fd events
- **`timerfd_create`(283), `timerfd_settime`(286)** — timer events as fds
- **`eventfd2`(290)** — inter-thread notification
- **`inotify_init1`(294), `inotify_add_watch`(254)** — filesystem monitoring

OSv has a solid epoll implementation (~380 lines in `core/epoll.cc`) and timerfd support
that we can port. But signalfd, eventfd, and inotify have no permissive reference — we
build these ourselves.

systemd also needs:
- **`mount`(165), `umount2`(166)** — mount proc, sys, cgroup, tmpfs
- **`sendmsg`(46), `recvmsg`(47)** — SCM_RIGHTS fd passing over Unix sockets (D-Bus)
- **`prctl`(157)** — PR_SET_CHILD_SUBREAPER, PR_SET_NAME
- **`capget`(125), `capset`(126)** — Linux capabilities
- **`memfd_create`(319)** — anonymous memory files
- **`name_to_handle_at`(303)** — file handle operations for device tracking

The `sendmsg`/`recvmsg` with SCM_RIGHTS ancillary data is essential — D-Bus (which is
required by systemd) passes file descriptors between processes this way. OSv's Unix domain
socket is too minimal (socketpair only, no named sockets, no fd passing). We implement
this from scratch.

### M5: apt/dpkg (~120 syscalls, ~10 more)

Package management adds:
- **`xattr` family** (188-199) — extended attributes for security labels
- **`utimensat`(280)** — preserve timestamps during package install
- **`fallocate`(285)** — preallocate disk space
- **`statx`(332)** — extended stat (modern glibc uses this)
- **`copy_file_range`(326), `splice`(275)** — efficient kernel-side data copying
- **`fchownat`(260), `fchmodat`(268)** — set ownership/permissions during install

OSv has `utimensat` and `fallocate` in its VFS. The `xattr` family and `splice`/`copy_file_range`
are implemented from scratch.

### M6: Full Networking (~130 syscalls, ~5 more)

Kevlar already has TCP/IP via smoltcp. The gaps are:
- **`accept4`(288)** — accept with SOCK_CLOEXEC (every server uses this)
- **`setsockopt`(54)** — SO_REUSEADDR, TCP_NODELAY, SO_KEEPALIVE
- **`recvmmsg`(299), `sendmmsg`(307)** — batch message I/O for high-throughput servers

Plus **AF_NETLINK** socket support — systemd, `ip`, and DNS resolution all need it.

### M7: Container Runtime (~145 syscalls, ~15 more)

Docker/containerd needs Linux namespaces:
- **`unshare`(272), `setns`(308)** — create and join namespaces
- **`pivot_root`(155)** — change root for container
- **`seccomp`(317)** — BPF-based syscall filtering
- **`clone3`(435)** — modern clone with CLONE_NEWPID, CLONE_NEWNET, etc.
- **`bpf`(321)** — eBPF for container networking

The new mount API (`open_tree`, `move_mount`, `fsopen`, `fsconfig`, `fsmount`) is also needed
by modern container runtimes.

### M8: Kubuntu 24.04 Desktop (~170 syscalls, ~25 more)

The final frontier adds graphics, audio, and desktop IPC:
- **SysV IPC** (`shmget`/`shmat`/`shmctl`/`shmdt`, `semget`/`semop`/`semctl`) — X11 MIT-SHM
- **`ptrace`(101)** — debuggers, strace
- **Scheduler** (`sched_setaffinity`, `sched_setscheduler`, etc.) — real-time audio
- **`io_uring`** (425-427) — modern async I/O
- **Memory protection keys** (`pkey_mprotect`, `pkey_alloc`, `pkey_free`)

Most of the actual graphics work is in ioctl commands on `/dev/dri/*` (KMS/DRM),
not in new syscalls. Wayland compositors use `sendmsg` with SCM_RIGHTS for buffer
passing — which we already need for systemd.

## Reference Architecture: FreeBSD + OSv

> **Update (2026-03-08):** After completing M1, we identified FreeBSD as a far superior
> reference for Linux syscall semantics. The original OSv assessment below is retained
> for context, but FreeBSD is now our primary reference.

### FreeBSD: The Ideal Reference

FreeBSD's `linuxulator` (`sys/compat/linux/`) is a complete, battle-tested Linux syscall
compatibility layer maintained under the BSD-2-Clause license. FreeBSD developers have
already solved exactly the problem Kevlar is solving: making Linux binaries run correctly
on a non-Linux kernel.

| Subsystem | FreeBSD Source | Quality for Kevlar |
|-----------|---------------|-------------------|
| Linux syscall semantics | sys/compat/linux/ | Excellent — maps every Linux syscall to correct behavior |
| VM management (mmap, mprotect, futex) | sys/vm/ | Excellent — production-grade, multiarch |
| Process/threading (clone, fork, signals) | sys/kern/ | Excellent — full POSIX + Linux extensions |
| Socket layer (AF_UNIX, sendmsg, SCM_RIGHTS) | sys/kern/uipc_* | Excellent — complete implementation |
| IPC (SysV shm/sem/msg, epoll, signalfd) | sys/compat/linux/ | Excellent — Linux-specific semantics |
| Namespaces, seccomp, capabilities | sys/compat/linux/ | Good — partial but growing |

**Why FreeBSD over OSv?** OSv is a unikernel designed for cloud VMs — it has no process
model, no fork, minimal signals, and no Linux-specific features. FreeBSD is a full
multi-process POSIX OS with a dedicated Linux compatibility layer. For everything beyond
basic VFS operations, FreeBSD is categorically the better reference.

**Clean-room safety:** Re-implementing FreeBSD's C code in Rust is a language transformation
of high-level concepts, not code copying. The BSD-2-Clause license explicitly permits study
and adaptation. This gives Kevlar a provably clean-room path to Linux compatibility.

### OSv: Still Useful for VFS

OSv (BSD-3-Clause) remains our reference for filesystem abstractions:

| Subsystem | OSv Quality | Usable for Kevlar? |
|-----------|------------|---------------------|
| VFS layer (vnode, mount, dentry) | Excellent (~2000 lines) | Yes — clean design, most file ops |
| nanosleep, clock_gettime, timerfd | Good | Yes — clean time subsystem |
| mmap/VMA management | Good (~2100 lines) | Partially — VMA logic useful, page tables arch-specific |

For everything else — threading, signals, futex, clone, Unix domain sockets, epoll,
namespaces, seccomp, inotify, signalfd, eventfd — FreeBSD is the reference.

## Implementation Strategy

We're not implementing all 170 syscalls at once. The plan is milestone-driven:

1. **M1 first.** Get static BusyBox running. This is the proof that the kernel works.
   Most of the work is in `mprotect`/`munmap` (memory management) and a batch of
   straightforward VFS calls.

2. **M2 immediately after.** Dynamic linking unlocks the entire Linux ecosystem. The hard
   part here is `futex` — without it, no dynamically-linked program can use threads or
   even initialize glibc properly.

3. **M3 is the capability cliff.** `clone` with full flag support is the single hardest
   syscall in the Linux API. Once we have it, plus job control signals, we can run real
   interactive shells.

4. **M4 is the distribution gate.** systemd is non-negotiable for any modern distro.
   The epoll event loop and fd-based event sources (signalfd, timerfd, eventfd, inotify)
   are a coherent subsystem that we can build together.

5. **M5-M8 are incremental.** Each one adds capability but the core architecture is
   established by M4.

We track every syscall's status, milestone assignment, and reference source in
[compatibility.md](../Documentation/compatibility.md). Each milestone gets its own
integration test: a real binary (BusyBox, bash, systemd) running on QEMU and verifying
that the expected commands work.

## What's Next

> **Update:** M1 is complete. BusyBox boots and runs an interactive shell. See
> [blog post 003](003-milestone-1-busybox-boots.md) for the full story.

Next milestones:
- **M1.5: ARM64 support** — boot.S is only ~250 lines, trap.S ~110, usercopy.S ~64. ARM64 parity is feasible.
- **M2: Dynamic linking** — `pread64`, `futex`, `madvise` for ld-linux.so. FreeBSD's `sys/vm/` and `sys/compat/linux/linux_futex.c` are the references.
- **M3: Coreutils + Bash** — `clone` with full flag support, job control. FreeBSD's `sys/compat/linux/linux_fork.c` maps the approach.
