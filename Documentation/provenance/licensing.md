# Licensing

Kevlar is tri-licensed under **MIT OR Apache-2.0 OR BSD-2-Clause**.

## License Compatibility

| Source Project | License | Compatible? | Usage in Kevlar |
|----------------|---------|-------------|-----------------|
| Kerla | MIT OR Apache-2.0 | Yes (sublicensed) | Fork base |
| smoltcp | 0-clause BSD | Yes | TCP/IP networking library (Cargo dependency) |
| Linux kernel | GPL-2.0 | No (code) | Man pages and POSIX specs only; never copy implementation |

## Clean-Room Approach

Kevlar is a clean-room implementation of the Linux ABI. When implementing syscalls,
the authoritative references are:

1. Linux man pages — for syscall interface specifications and behavior
2. POSIX standards — for standard semantics
3. Hardware specifications (ext2 spec, Intel SDM, ARM ARM) — for device/format details

No proprietary or GPL-licensed source code is consulted for implementation.

## Why Not GPL?

Kevlar's goal is a fully permissively licensed kernel that can be used in any
context, including proprietary products. GPL-2.0 (Linux) requires derivative works
to be GPL-licensed, which does not fit our use case.
