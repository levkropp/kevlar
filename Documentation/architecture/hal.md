# Platform / HAL

The `kevlar_platform` crate is Ring 0 in the ringkernel architecture. It is the only
crate that may contain `unsafe` code; everything above it uses `#![deny(unsafe_code)]`
or `#![forbid(unsafe_code)]`.

For the design philosophy and ring boundary details, see
[The Ringkernel Architecture](ringkernel.md).

## What the Platform Does

The platform crate owns every operation that requires direct hardware access:

| Subsystem | Responsibility |
|---|---|
| Paging | Physical frame allocation, page table construction, TLB flushes |
| Context switch | Saving/restoring task state (GP registers, xsave FPU area) |
| User-kernel copy | `copy_from_user` / `copy_to_user` with `access_ok()` validation |
| IRQ | IDT setup, APIC EOI, IRQ routing |
| Boot | GDT, TSS, SYSCALL/SYSRET MSRs, EFER (LME|NXE) |
| Timer | APIC timer calibration via TSC; `TICK_HZ = 100` |
| TSC clock | Fixed-point nanosecond conversion (`ns = (delta * NS_MULT) >> 32`) |
| vDSO | 4 KB ELF shared object mapped into every process for fast `clock_gettime` |
| Randomness | RDRAND / RDSEED wrappers |
| Memory ops | Custom `memcpy`, `memset`, `memcmp` (no SSE; kernel runs with SSE disabled) |

## Key Types

### `OwnedFrame`

Represents exclusive ownership of one physical page frame. The frame is zeroed on
allocation and freed on drop.

Under the **Fortress** safety profile, `OwnedFrame` exposes only `read()`/`write()`
copy operations — safe code can never hold a `&mut [u8]` into physical memory.
Under **Balanced** and other profiles, `page_as_slice_mut` is available for
performance-critical paths.

### `UserPtr<T: Pod>`

A validated user-space address. Can only be constructed after `access_ok()` bounds
checking. `Pod` constrains `T` to `Copy + repr(C)` types, preventing references or
types with drop glue from crossing the user-kernel boundary.

### `Task`

An opaque kernel task: kernel stack, saved general-purpose registers, and a 512-byte
xsave area for FPU/SSE state. Context switches save and restore the full xsave area.

`fork()` copies the parent's xsave area to the child to preserve FPU state across fork.

## User-Mode Entry

```
enter_usermode(task) → UserEvent
```

The platform returns control to the Core by returning a `UserEvent` value, not by
calling up into the Core. This keeps the call direction unambiguous and prevents
the Core from needing to register callbacks with the platform.

```rust
pub enum UserEvent {
    Syscall { number: usize, args: [usize; 6] },
    PageFault { addr: UserVAddr, write: bool },
    Interrupt { vector: u8 },
    Signal,
}
```

## Architecture Variants

The platform crate has separate modules for x86\_64 (`platform/x64/`) and ARM64
(`platform/arm64/`). Both expose the same safe API to the kernel.

| Feature | x86\_64 | ARM64 |
|---|---|---|
| Syscall entry | SYSCALL/SYSRET MSRs | SVC instruction |
| Timer | APIC + TSC calibration | ARM generic timer (CNTFRQ\_EL0) |
| Interrupt controller | APIC (QEMU q35) | GIC-v2 (QEMU virt) |
| vDSO | Yes (`platform/x64/vdso.rs`) | Not yet |
| QEMU target | `q35 -cpu Icelake-Server` | `virt -cpu cortex-a72` |

## Safety Model

The platform crate enforces safety through:

1. **No public raw pointer APIs.** All pointer-taking functions return `Result` and
   validate bounds before any dereference.
2. **`Pod` constraint on user copies.** Prevents references from crossing the boundary.
3. **SAFETY comments.** Every `unsafe` block in the platform crate has a `// SAFETY:`
   comment explaining the invariant that makes it sound.
4. **`access_ok()` on all user addresses.** Skipped only in the Ludicrous profile.

The platform crate currently has ~7 `unsafe` sites across 4 files, all with explicit
`#[allow(unsafe_code)]` annotations and SAFETY comments.
