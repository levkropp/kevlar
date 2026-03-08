# Licensing

Kevlar is dual-licensed under **MIT OR Apache-2.0**, matching the original Kerla upstream license.

## License Compatibility

| Source Project | License | Compatible? | Usage in Kevlar |
|----------------|---------|-------------|-----------------|
| Kerla | MIT OR Apache-2.0 | Yes (identical) | Fork base, all original kernel code |
| OSv | BSD-3-Clause | Yes | Port C/C++ implementations to Rust with attribution |
| Asterinas | MPL-2.0 | Design only | Study architecture and feature lists; NO code copying |
| Linux kernel | GPL-2.0 | No (code) | Read man pages and POSIX specs only; never copy implementation |
| smoltcp | 0-clause BSD | Yes | TCP/IP networking library (Cargo dependency) |

## BSD-3-Clause Compatibility

BSD-3-Clause code (from OSv) is fully compatible with both MIT and Apache-2.0.
When porting OSv code to Rust, we:

1. Retain the original copyright notice in the file
2. Add an entry to the `NOTICE` file at the repository root
3. Document the port in the [Clean-Room Implementation Log](clean-room-log.md)

## Why Not MPL-2.0 / GPL?

Kevlar's goal is a fully permissively licensed kernel that can be used in any context,
including proprietary products. MPL-2.0 (Asterinas) requires modifications to MPL-licensed
files to remain under MPL. GPL-2.0 (Linux) requires derivative works to be GPL-licensed.
Neither fits our use case.
