# Source Attribution

This document tracks the provenance of all code in Kevlar.

## Kerla (MIT OR Apache-2.0)

The entire initial codebase is forked from [Kerla](https://github.com/nuta/kerla) by Seiya Nuta.
All files originating from Kerla are covered by the MIT OR Apache-2.0 dual license.

**Scope:** All files present at initial fork (Phase 0).

## OSv (BSD-3-Clause)

Portions of the following subsystems are ported from [OSv](https://github.com/cloudius-systems/osv)
(Copyright Cloudius Systems, BSD-3-Clause) by translating C/C++ implementations to Rust:

| Subsystem | OSv Source | Kevlar Destination | Phase |
|-----------|-----------|-------------------|-------|
| *To be filled as ports are completed* | | | |

## FreeBSD (BSD-2-Clause)

The following subsystems reference [FreeBSD](https://github.com/freebsd/freebsd-src)
(Copyright The FreeBSD Project, BSD-2-Clause) for syscall semantics and implementation approach:

| Subsystem | FreeBSD Source | Kevlar Destination | Phase |
|-----------|---------------|-------------------|-------|
| Linux syscall semantics | sys/compat/linux/ | kernel/syscalls/ | M1+ |
| VM management | sys/vm/ | kernel/mm/ | M1+ |
| Process/signal handling | sys/kern/kern_sig.c | kernel/process/ | M2+ |
| *To be filled as implementations reference FreeBSD* | | | |

Note: Kevlar does not copy FreeBSD code. We study FreeBSD's implementation approach and
re-implement the concepts in Rust. This constitutes a clean-room language transformation.

## Original Code

All code not attributed to Kerla or OSv is original work by Kevlar contributors,
licensed under MIT OR Apache-2.0.

## Asterinas (MPL-2.0) - Design Reference Only

Asterinas was studied for architectural patterns and feature completeness.
**No Asterinas code was copied into Kevlar.** The following design concepts were
informed by studying Asterinas's public API and documentation:

| Concept | Asterinas Reference | Kevlar Implementation |
|---------|--------------------|-----------------------|
| Framekernel (safe/unsafe split) | `ostd/` vs `kernel/` separation | HAL/kernel split (original implementation) |
| *To be filled as features are implemented* | | |
