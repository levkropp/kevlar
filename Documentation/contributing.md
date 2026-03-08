# Contributing to Kevlar

## License

All contributions must be dual-licensed under MIT OR Apache-2.0.

## Clean-Room Requirements

When implementing new subsystems:

1. **Document your references** in [clean-room-log.md](provenance/clean-room-log.md)
2. **Never copy** Asterinas (MPL-2.0) or Linux (GPL-2.0) code
3. **OSv code** (BSD-3-Clause) may be ported to Rust with proper attribution
4. **Add SPDX headers** to all new `.rs` files: `// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause`

## Code Style

- Safe Rust in `kernel/` (`#![deny(unsafe_code)]`)
- All unsafe code goes in `runtime/` (HAL)
- Every `unsafe` block requires a `// SAFETY:` comment
- Use `log` crate macros for logging (no `println!`)
- Error handling with `Result<T>` and `?` operator
