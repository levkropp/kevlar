# Panic Containment and Capability Tokens

**Date:** 2026-03-08

---

This post covers the final two infrastructure phases of Kevlar's safety profile system: `catch_unwind` for panic containment (Phase 5) and capability tokens at ring boundaries (Phase 6).

## Phase 5: catch_unwind — the hard part

The promise of the ringkernel: a panicking filesystem returns `EIO` instead of crashing the kernel. That requires `catch_unwind`, which requires stack unwinding, which requires `.eh_frame` tables and a bare-metal unwinder.

Most Rust kernels compile with `panic = "abort"` — smaller binaries, no unwind overhead. We need both modes: unwind for Fortress/Balanced (panic containment), abort for Performance/Ludicrous (maximum speed).

### Dual target specs

We now have two target specifications per architecture:

- `kernel/arch/x64/x64.json` — `"panic-strategy": "abort"` (Performance, Ludicrous)
- `kernel/arch/x64/x64-unwind.json` — `"panic-strategy": "unwind"` (Fortress, Balanced)

The Makefile selects the target based on `PROFILE`:

```makefile
ifeq ($(filter $(PROFILE),fortress balanced),$(PROFILE))
target_json := kernel/arch/$(ARCH)/$(ARCH)-unwind.json
else
target_json := kernel/arch/$(ARCH)/$(ARCH).json
endif
```

### Dual linker scripts

The abort linker script discards `.eh_frame` sections — useless overhead when unwinding is disabled. The unwind linker script preserves them and exports the symbols the unwinder needs:

```ld
/* x64-unwind.ld */
.eh_frame : AT(ADDR(.eh_frame) - VMA_OFFSET) {
    __eh_frame = .;
    KEEP(*(.eh_frame));
    KEEP(*(.eh_frame.*));
    __eh_frame_end = .;
}
```

### The unwinding crate

We use the [`unwinding`](https://crates.io/crates/unwinding) crate (v0.2, MIT/Apache-2.0) by Gary Guo — a pure Rust alternative to libgcc_eh that works in `no_std`. Features: `unwinder`, `fde-static`, `personality`, `panic`.

Key API:
- `unwinding::panic::begin_panic(payload)` — initiates stack unwinding
- `unwinding::panic::catch_unwind(f)` — catches panics, returns `Result<R, Box<dyn Any>>`

### Panic handler integration

Our `#[panic_handler]` now tries to unwind before crashing:

```rust
#[cfg(any(feature = "profile-fortress", feature = "profile-balanced"))]
{
    let msg = info.to_string();
    let _ = unwinding::panic::begin_panic(Box::new(msg));
    // If begin_panic returns, no catch frame was found.
    // Fall through to crash dump.
}
```

If a `catch_unwind` frame exists on the stack (i.e., the panic originated inside a service call), execution resumes there. If not, `begin_panic` returns and we proceed with the existing crash dump logic.

### Service call wrapper

The `call_service()` function wraps service calls with `catch_unwind`:

```rust
// Fortress/Balanced: catch panics at ring boundary
pub fn call_service<R>(f: impl FnOnce() -> Result<R>) -> Result<R> {
    match unwinding::panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(payload) => {
            warn!("service panicked, returning EIO: {}", msg);
            Err(Errno::EIO.into())
        }
    }
}

// Performance/Ludicrous: zero overhead
#[inline(always)]
pub fn call_service<R>(f: impl FnOnce() -> Result<R>) -> Result<R> { f() }
```

Under Performance/Ludicrous, `call_service` compiles to nothing — the closure is inlined at the call site.

## Phase 6: Capability tokens

Capabilities prove that a service is authorized to perform an operation. The kernel core mints tokens during service registration; services must hold the token to access privileged resources.

### Three implementations, one API

```rust
// platform/capabilities.rs

// Fortress: runtime-validated, carries a random nonce
pub struct Cap<T> { nonce: u64, _marker: PhantomData<T> }

// Balanced: zero-cost newtype, erased at compile time
pub struct Cap<T> { _marker: PhantomData<T> }

// Performance/Ludicrous: zero-size, always valid
pub struct Cap<T> { _marker: PhantomData<T> }
```

Under Fortress, `mint()` generates a unique nonce and `validate()` checks it — a forged token with the wrong nonce is rejected. Under Balanced, the type system does the enforcement: only code that receives a `Cap<NetAccess>` from the core can call functions requiring it. Under Performance/Ludicrous, tokens exist only to keep the API uniform — they compile away entirely.

### Current capabilities

- `Cap<NetAccess>` — permission to send/receive network frames
- `Cap<PageAlloc>` — permission to allocate physical pages
- `Cap<BlockAccess>` — permission to access block devices

The network stack service receives `Cap<NetAccess>` at registration. Under Fortress, the token is validated on each `network_stack()` call via `debug_assert!`.

## Status

All seven implementation phases are now complete or in progress:

| Phase | What | Status |
|-------|------|--------|
| 0 | Feature flags and Makefile | Done |
| 1 | Performance profile (concrete types) | Done |
| 2 | Ludicrous profile (skip access_ok) | Done |
| 3 | Optimized usercopy | Done |
| 4 | Copy-semantic frames (Fortress) | Done |
| 5 | catch_unwind at service boundaries | Done |
| 6 | Capability tokens | Done |
| 7 | Benchmarks and CI matrix | Next |

Every profile compiles and boots. The infrastructure is in place. What remains is measuring the cost of each safety mechanism and expanding the capability system as more services are extracted.
