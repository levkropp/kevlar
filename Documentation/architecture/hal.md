# HAL / Kernel Split

*This section will be written during Phase 0.5.*

Kevlar uses a **framekernel-inspired architecture** where all `unsafe` code is confined
to the Hardware Abstraction Layer (HAL, formerly `runtime/`), and the kernel crate
uses `#![deny(unsafe_code)]` to enforce safety at compile time.

## Design Goals

- **Small trusted computing base**: Only the HAL contains `unsafe` code
- **Safe kernel development**: All syscall handlers, filesystem logic, process management,
  and networking code is written in safe Rust
- **Clear API boundary**: The HAL exposes safe abstractions that the kernel consumes

## Key Abstractions

| HAL Type | Purpose |
|----------|---------|
| `VmSpace` | User address space (map, unmap, protect pages) |
| `Task` | Schedulable execution context |
| `UserContext` | Saved user-space registers |
| `WaitQueue` | Safe blocking and waking |
| `UserReader` / `UserWriter` | Safe user memory access |
