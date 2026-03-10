# Source Attribution

This document tracks the provenance of all code in Kevlar.

## Kerla (MIT OR Apache-2.0)

The entire initial codebase is forked from [Kerla](https://github.com/nuta/kerla)
by Seiya Nuta. All files originating from Kerla are covered by the MIT OR Apache-2.0
dual license.

**Scope:** All files present at initial fork (Phase 0).

## FreeBSD (BSD-2-Clause)

The following subsystems reference [FreeBSD](https://github.com/freebsd/freebsd-src)
(Copyright The FreeBSD Project, BSD-2-Clause) for syscall semantics and implementation
approach:

| Subsystem | FreeBSD Source | Kevlar Destination |
|-----------|---------------|-------------------|
| Linux syscall semantics | `sys/compat/linux/` | `kernel/syscalls/` |
| VM management | `sys/vm/` | `kernel/mm/` |
| Process/signal handling | `sys/kern/kern_sig.c` | `kernel/process/` |

Note: Kevlar does not copy FreeBSD code. We study FreeBSD's implementation approach
and re-implement the concepts in Rust. This constitutes a clean-room language
transformation, not copying.

## Original Code

All code not attributed to Kerla or FreeBSD is original work by Kevlar contributors,
licensed under MIT OR Apache-2.0.
