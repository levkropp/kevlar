# Clean-Room Implementation Log

This log documents every subsystem implementation, recording what references were
consulted and how the implementation was derived. This serves as legal protection
and demonstrates clean-room discipline.

---

## Phase 0: Fork and Modernize - 2026-03-08

### Reference materials consulted
- Kerla source code (MIT OR Apache-2.0) - direct fork
- Rust Edition 2024 migration guide

### Implementation approach
Forked Kerla, renamed all references from `kerla` to `kevlar`, updated Rust toolchain
and dependencies to modern versions.

### Attribution
- All code from Kerla (Copyright 2021 Seiya Nuta, MIT OR Apache-2.0)

### Test coverage
- Build verification
- QEMU boot test

---

## Phase 1: Milestone 1 - Static Busybox - 2026-03-08

### Reference materials consulted
- Kerla source code (MIT OR Apache-2.0) — existing syscall implementations
- OSv source code (BSD-3-Clause) — VFS layer for lseek, rename, unlink, rmdir, mprotect/munmap VMA logic
- Linux man pages — POSIX syscall specifications
- smoltcp documentation — networking API migration (0.7 → 0.12)
- FreeBSD linuxulator source (BSD-2-Clause) — identified as primary reference for future syscall work

### Implementation approach
Added 44 new syscalls to reach 79 total. Upgraded all dependencies including major smoltcp migration.
Fixed critical boot bugs: EFER.NXE for NX page protection, custom memcpy/memset/memcmp for no-SSE kernel.
Established FreeBSD as primary reference for Linux syscall semantics going forward.

### New syscalls implemented
lseek, mprotect, munmap, openat, newfstatat, dup, dup3, pipe2, access, vfork, sched_yield,
umask, getegid, getpgrp, getgid, unlink, rmdir, rename, nanosleep, gettimeofday, getrlimit,
sysinfo, mmap (prot flags), select (writefds fix), wait4 (status encoding fix)

### Attribution
- Memory management VMA logic influenced by OSv core/mmu.cc (BSD-3-Clause)
- VFS operations influenced by OSv fs/vfs/vfs_syscalls.cc (BSD-3-Clause)
- Boot sequence and syscall dispatch from Kerla (MIT OR Apache-2.0)

### Test coverage
- QEMU boot test: BusyBox shell interactive (echo, ls, cat verified)
- Syscall trace logging (all 79 syscalls verified in boot sequence)

---

## Phase 2: Milestone 2 - Dynamic Linking - 2026-03-08

### Reference materials consulted
- Linux man pages — ELF loading (execve(2), elf(5)), mmap semantics, auxiliary vectors
- musl libc source (MIT) — dynamic linker behavior, `reclaim_gaps` allocator
- ELF specification — PT_INTERP, PT_LOAD, AT_* auxiliary vector entries

### Implementation approach
Extended ELF loader for PIE (ET_DYN) and dynamic linking. Kernel loads main executable, detects
PT_INTERP, loads interpreter as second ELF image, constructs auxiliary vector, and jumps to
interpreter entry point. Three critical bugs fixed: page fault handler VMA offset, AT_PHDR
relocation for PIE, gap-fill VMAs for musl's reclaim_gaps.

### New syscalls implemented
pread64 (full), madvise (stub), futex (partial: FUTEX_WAIT/FUTEX_WAKE), set_robust_list (stub),
set_tid_address (fixed)

### Attribution
- ELF loading: Own implementation from ELF spec and man pages
- MAP_FIXED semantics: Own implementation from mmap(2) man page

### Test coverage
- Dynamically linked hello-world (/bin/hello-dynamic) boots and runs
- Static BusyBox still boots correctly (regression test)

---

## Phase 3: M3 Preparation - Bug Fixes + Terminal Control + *at Syscalls - 2026-03-08

### Reference materials consulted
- POSIX signal(7) man page — default signal dispositions
- POSIX termios(3), tty_ioctl(4) man pages — terminal attributes
- Linux asm-generic/termbits.h — struct termios layout (36 bytes, NCCS=19)
- POSIX man pages for: kill(2), sigaction(2), setsid(2), getsid(2), symlink(2),
  symlinkat(2), unlinkat(2), mkdirat(2), renameat(2), readlinkat(2), readv(2),
  pwrite(2), ftruncate(2), fchdir(2), getrusage(2), sigaltstack(2), getpgid(2)

### Implementation approach
Phase 0 fixed critical existing bugs: signal defaults (most were Ignore instead of
Terminate per POSIX), O_EXCL logic in open(2), kill(2) for pid<-1, rt_sigaction oldact
support, O_TRUNC/O_APPEND handling, fcntl F_DUPFD.

Phase 1 rewrote the Termios struct to match the Linux kernel ABI (c_iflag/c_oflag/c_cflag/
c_lflag/c_line/c_cc[19]). Added TCGETS/TCSETS/TIOCGWINSZ/TIOCSWINSZ ioctls to serial TTY,
PTY master, and PTY slave. Added ^Z (SIGTSTP) and ^D (EOF) handling in line discipline.
Added WinSize struct. Added setsid/getsid syscalls.

Phase 2-4 added symlink support to tmpfs (TmpFsSymlink type, create_symlink, lookup,
readdir, unlink handling). Implemented *at syscalls (unlinkat, mkdirat, renameat,
readlinkat, symlinkat) with dirfd-relative path resolution. Added ftruncate, pwrite64,
readv, fchdir, getrusage (stub), sigaltstack (stub). Fixed getpgid for non-zero pid.

### New syscalls and features
- Bug fixes: DEFAULT_ACTIONS, O_EXCL, O_TRUNC, O_APPEND, kill pid<-1, rt_sigaction oldact, fcntl F_DUPFD
- Terminal: TCGETS, TCSETS, TCSETSW, TCSETSF, TIOCGWINSZ, TIOCSWINSZ, TIOCGPTN (PTY master)
- Session: setsid, getsid
- Filesystem: symlink, symlinkat, unlinkat, mkdirat, renameat/renameat2, readlinkat, fchdir, ftruncate
- IO: pwrite64, readv
- Stubs: getrusage, sigaltstack
- Fixes: getpgid (non-zero pid)

### Provenance per syscall
| Syscall | Provenance | Notes |
|---------|-----------|-------|
| signal defaults | Own | POSIX signal(7) man page |
| O_EXCL fix | Own | POSIX open(2) man page |
| O_TRUNC/O_APPEND | Own | POSIX open(2)/write(2) man pages |
| kill pid<-1 | Own | POSIX kill(2) man page |
| rt_sigaction oldact | Own | POSIX sigaction(2) man page |
| fcntl F_DUPFD | Own | POSIX fcntl(2) man page |
| TCGETS/TCSETS | Own | Linux tty_ioctl(4), asm-generic/termbits.h |
| TIOCGWINSZ | Own | Linux tty_ioctl(4) |
| setsid/getsid | Own | POSIX setsid(2)/getsid(2) man pages |
| symlink/symlinkat | Own | POSIX symlink(2)/symlinkat(2) man pages |
| unlinkat | Own | POSIX unlinkat(2) man page |
| mkdirat | Own | POSIX mkdirat(2) man page |
| renameat | Own | POSIX renameat(2) man page |
| readlinkat | Own | POSIX readlinkat(2) man page |
| fchdir | Own | POSIX fchdir(2) man page |
| ftruncate | Own | POSIX ftruncate(2) man page |
| pwrite64 | Own | POSIX pwrite(2) man page |
| readv | Own | POSIX readv(2) man page |
| getrusage | Own (stub) | POSIX getrusage(2) man page |
| sigaltstack | Own (stub) | POSIX sigaltstack(2) man page |
| getpgid fix | Own | POSIX getpgid(2) man page |

### Test coverage
- QEMU boot test: BusyBox shell boots, echo/ls work
- Dynamic linking: /bin/hello-dynamic still works (regression test)

---

## Phase 4: M3 Continued — Job Control, Clone, Additional Stubs - 2026-03-08

### Reference materials consulted
- POSIX signal(7) man page — SIGSTOP/SIGTSTP/SIGCONT semantics
- Linux wait(2) man page — wait status encoding (WIFSTOPPED, WIFCONTINUED)
- Linux clone(2) man page — clone flag bits (CLONE_VM, CLONE_THREAD, etc.)
- POSIX tgkill(2), pause(2), alarm(2), sigsuspend(2) man pages
- POSIX fchmod(2), fchown(2), getgroups(2) man pages

### Implementation approach
Phase 5 added job control infrastructure: `Stopped(Signal)` process state, `Stop` and
`Continue` SigAction variants, SIGCONT handling in send_signal (continues stopped processes),
WUNTRACED support in wait4 (reports stopped children with correct status encoding).

Phase 6 added clone syscall with proper flag parsing (rejects CLONE_VM/CLONE_THREAD with
ENOSYS, handles fork-like clones from musl), plus stub syscalls needed by Bash/coreutils:
tgkill, rt_sigsuspend, pause, alarm, fchmod/fchmodat/fchownat, getgroups.

### New syscalls and features
- Job control: ProcessState::Stopped, SigAction::Stop/Continue, SIGSTOP/SIGTSTP/SIGCONT
- wait4: WUNTRACED support, correct stopped/exited status encoding
- clone: proper flag handling (fork-like for musl, ENOSYS for threading)
- tgkill: signal individual threads (currently == kill by tid)
- rt_sigsuspend: temporarily replace signal mask and wait for signal
- pause: wait for signal delivery
- alarm: stub (returns 0, no timer delivery yet)
- fchmod/fchmodat/fchownat: stub (succeed silently on tmpfs)
- getgroups: stub (returns 0 supplementary groups)

### Provenance per syscall
| Syscall | Provenance | Notes |
|---------|-----------|-------|
| ProcessState::Stopped | Own | POSIX signal(7) man page |
| SigAction::Stop/Continue | Own | POSIX signal(7) man page |
| SIGCONT handling | Own | POSIX signal(7) man page |
| wait4 WUNTRACED | Own | Linux wait(2) man page |
| wait status encoding | Own | Linux wait(2) man page |
| clone | Own | Linux clone(2) man page |
| tgkill | Own | POSIX tgkill(2) man page |
| rt_sigsuspend | Own | POSIX sigsuspend(2) man page |
| pause | Own | POSIX pause(2) man page |
| alarm | Own (stub) | POSIX alarm(2) man page |
| fchmod/fchmodat | Own (stub) | POSIX fchmod(2) man page |
| fchownat | Own (stub) | POSIX fchown(2) man page |
| getgroups | Own (stub) | POSIX getgroups(2) man page |

### Test coverage
- QEMU boot test: BusyBox shell boots, echo/ls work
- Dynamic linking still works (regression test)

---

*Subsequent phases will add entries here as subsystems are implemented.*
