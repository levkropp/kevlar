# Contributing to Kevlar

## License

All contributions must be licensed under **MIT OR Apache-2.0 OR BSD-2-Clause**.
Add an SPDX header to every new `.rs` file:

```rust
// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
```

## Clean-Room Requirements

Kevlar is a clean-room implementation of the Linux ABI:

1. **Use Linux man pages and POSIX specifications** as the primary reference for
   syscall semantics
2. **Never copy** GPL-licensed kernel code (Linux, RTEMS, etc.)
3. **Man pages** are always safe to reference for interface specifications

## Code Style

- **Safe Rust in `kernel/`** — the kernel crate enforces `#![deny(unsafe_code)]`
- **All unsafe code goes in `platform/`** — every `unsafe` block requires a
  `// SAFETY:` comment explaining the invariant
- **Service crates** (`services/`, `libs/kevlar_vfs/`) use `#![forbid(unsafe_code)]`
- Use `log` crate macros for logging — no `println!`
- Error handling with `Result<T>` and the `?` operator
- No `unwrap()` in kernel paths — propagate errors or use `expect` with a message

## Architecture Rules

Follow the ringkernel trust boundaries:

- Hardware access only in `platform/` (Ring 0)
- OS policies in `kernel/` (Ring 1)
- Pluggable services in `services/` (Ring 2)
- Shared VFS types in `libs/kevlar_vfs/` (no kernel dependencies)

If a change requires adding `unsafe` code outside `platform/`, discuss it first.

## Testing

```
make run                   # Boot and check the shell works
make check                 # Quick type-check
make check-all-profiles    # Verify all safety profiles build
make bench                 # Run benchmarks (should not regress)
```

There is no automated test runner yet beyond the benchmarks. Boot the kernel and
exercise the affected subsystem manually.
