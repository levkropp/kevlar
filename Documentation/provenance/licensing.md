# Licensing

Kevlar is tri-licensed under **MIT OR Apache-2.0 OR BSD-2-Clause**. The original
Kerla upstream was MIT OR Apache-2.0; BSD-2-Clause was added to align with
FreeBSD, the primary reference for syscall semantics.

## License Compatibility

| Source Project | License | Compatible? | Usage in Kevlar |
|----------------|---------|-------------|-----------------|
| Kerla | MIT OR Apache-2.0 | Yes (identical) | Fork base, all original kernel code |
| FreeBSD | BSD-2-Clause | Yes | Reference for Linux compat layer (linuxulator) and POSIX syscall semantics |
| smoltcp | 0-clause BSD | Yes | TCP/IP networking library (Cargo dependency) |
| Linux kernel | GPL-2.0 | No (code) | Man pages and POSIX specs only; never copy implementation |

## FreeBSD as Primary Reference

FreeBSD's linuxulator (`sys/compat/linux/`) is a complete Linux syscall compatibility
layer maintained by the FreeBSD project under the BSD-2-Clause license. It provides:

1. **Battle-tested Linux syscall semantics** — FreeBSD developers have mapped every
   Linux syscall to its correct behavior, including edge cases and error conditions
2. **Clean-room safety** — Re-implementing FreeBSD's C code in Rust constitutes a
   language transformation, not code copying. The BSD license permits both study and
   adaptation.
3. **Linux-focused perspective** — FreeBSD's linuxulator specifically targets Linux
   binary compatibility, exactly matching Kevlar's goals

When implementing a new syscall, the recommended reference order is:
1. FreeBSD linuxulator (`sys/compat/linux/`) — for Linux-specific semantics
2. FreeBSD kernel (`sys/kern/`, `sys/vm/`) — for POSIX-standard implementations
3. Linux man pages — for POSIX specification details (never the implementation)

## BSD Compatibility

When porting BSD-licensed code to Rust, we:

1. Retain the original copyright notice in the file
2. Add an entry to the `NOTICE` file at the repository root
3. Document the port in the [Clean-Room Implementation Log](clean-room-log.md)

## Why Not GPL?

Kevlar's goal is a fully permissively licensed kernel that can be used in any
context, including proprietary products. GPL-2.0 (Linux) requires derivative works
to be GPL-licensed, which does not fit our use case.
