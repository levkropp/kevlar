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

*Subsequent phases will add entries here as subsystems are implemented.*
