# Safety Profiles

Kevlar is the first Linux-compatible kernel where you choose your safety level
at compile time. One Cargo feature flag controls how much safety overhead the
kernel pays, from fortress-grade fault isolation to bare-metal performance
that can beat Linux.

## The Four Profiles

```
                      Fortress   Balanced   Performance   Ludicrous
────────────────────────────────────────────────────────────────────
Rings                 3          3          2             1
catch_unwind          yes        yes        no            no
Service dispatch      dyn Trait  dyn Trait  concrete      concrete
Capability tokens     runtime    compile    none          none
access_ok() checks    yes        yes        yes           no
Copy-semantic frames  yes        no         no            no
Panic strategy        unwind     unwind     abort         abort
────────────────────────────────────────────────────────────────────
Unsafe %              ~3%        ~10%       ~10%          100%
Est. vs Linux         -15~25%    -5~10%     ~parity       +0~5%
Fault containment     service    service    kernel crash  kernel crash
```

### Fortress (`--features profile-fortress`)

Maximum safety. Every layer of protection enabled.

- **3 rings** with `catch_unwind` at every Ring 1 → Ring 2 call. A panicking
  filesystem or network stack returns `EIO` instead of crashing the kernel.
- **Copy-semantic page frames.** `OwnedFrame` exposes only `read()`/`write()`
  — safe code can never hold a `&mut [u8]` into physical memory. This
  eliminates an entire class of use-after-unmap bugs.
- **Runtime capability validation.** Service capability tokens carry a nonce
  checked at ring boundaries.
- **Byte-level usercopy.** Current assembly with full `access_ok()` validation.
- **Unsafe TCB: ~3%.** Only ~1,100 lines in the platform crate (boot, page
  tables, context switch, MMIO). `page_as_slice_mut` is removed entirely.

Best for: servers handling sensitive data, security-critical deployments.

### Balanced (`--features profile-balanced`) — **default**

The sweet spot. Safety where it matters, performance where it counts.

- **3 rings** with `catch_unwind`. Service panics are contained.
- **Direct-mapped page frames.** `page_as_slice_mut` returns `&'static mut [u8]`
  (current behavior). Fast, but safe code can hold dangling frame references.
- **Compile-time capability tokens.** Zero-cost newtypes erased at compile time.
- **Optimized usercopy.** Alignment-aware, `rep movsq` bulk copies.
- **Unsafe TCB: ~10%.** The full platform crate.

Best for: general-purpose use, development, most deployments.

### Performance (`--features profile-performance`)

Asterinas-equivalent safety at monolithic speed.

- **2 rings.** Services compile into the kernel as concrete types — no trait
  object vtable dispatch, no `catch_unwind`. The compiler monomorphizes and
  inlines service calls.
- **Direct-mapped page frames.**
- **No capability tokens.**
- **Optimized usercopy** with `access_ok()`.
- **Unsafe TCB: ~10%.** Same platform crate, same amount of unsafe code as
  Balanced. The difference is fault containment: a service panic crashes the
  kernel instead of returning `EIO`.

Best for: latency-sensitive workloads, benchmarking, when you trust your services.

### Ludicrous (`--features profile-ludicrous`)

Everything off. Potentially faster than Linux.

- **1 ring.** `#![allow(unsafe_code)]` everywhere. No ring boundaries.
- **No `access_ok()`.** User pointer validation relies entirely on the page
  fault handler (reactive, not proactive).
- **`get_unchecked()`** on proven-safe hot paths.
- **Optimized usercopy.**
- **Unsafe TCB: 100%.** All code is trusted.

Rust still provides memory safety within safe code (ownership, lifetimes,
bounds checking on most paths). This mode removes the *kernel-specific*
safety layers, not Rust's baseline guarantees. The performance advantage
over Linux comes from Rust's monomorphization, zero-cost abstractions,
and better aliasing information for the optimizer.

Best for: gaming/Wine workloads, maximum throughput, trusted environments.

## Usage

```bash
# Default (Balanced)
make run

# Select a profile
make run PROFILE=fortress
make run PROFILE=performance
make run PROFILE=ludicrous

# Check all profiles build
make check-all-profiles
```

## Implementation

### Feature flag ownership

The `kevlar_platform` crate owns the canonical feature flags. Higher crates
forward them via Cargo feature unification:

```toml
# platform/Cargo.toml
[features]
default = ["profile-balanced"]
profile-fortress = []
profile-balanced = []
profile-performance = []
profile-ludicrous = []
```

```toml
# kernel/Cargo.toml
[features]
default = ["kevlar_platform/profile-balanced"]
profile-fortress = ["kevlar_platform/profile-fortress"]
profile-balanced = ["kevlar_platform/profile-balanced"]
profile-performance = ["kevlar_platform/profile-performance"]
profile-ludicrous = ["kevlar_platform/profile-ludicrous"]
```

A `compile_error!` guard in `platform/lib.rs` ensures exactly one profile is
active.

### Panic strategy

Fortress and Balanced require `panic = "unwind"` for `catch_unwind` to work.
Performance and Ludicrous use `panic = "abort"` (current behavior).

This requires two target spec variants per architecture:
- `kernel/arch/x64/x64.json` — `"panic-strategy": "abort"` (Performance, Ludicrous)
- `kernel/arch/x64/x64-unwind.json` — `"panic-strategy": "unwind"` (Fortress, Balanced)

The Makefile selects the target spec based on `PROFILE`. The unwind variant
requires an `eh_personality` lang item and the `unwinding` crate (MIT/Apache-2.0).

### What changes per profile

| Mechanism | File | Fortress | Balanced | Performance | Ludicrous |
|-----------|------|----------|----------|-------------|-----------|
| `#![deny(unsafe_code)]` on kernel | `kernel/main.rs` | deny | deny | deny | allow |
| `#![forbid(unsafe_code)]` on services | `services/*/lib.rs` | forbid | forbid | forbid | allow |
| `catch_unwind` in service calls | `kernel/services.rs` | yes | yes | no | no |
| Service dispatch type | `kernel/services.rs` | `Arc<dyn Trait>` | `Arc<dyn Trait>` | `Arc<Concrete>` | `Arc<Concrete>` |
| `access_ok()` | `platform/address.rs` | check | check | check | no-op |
| `page_as_slice_mut` | `platform/page_ops.rs` | removed | available | available | available |
| `OwnedFrame` | `platform/page_ops.rs` | required | optional | N/A | N/A |
| Capability tokens | `platform/capabilities.rs` | runtime | compile-time | absent | absent |
| Panic strategy | target spec JSON | unwind | unwind | abort | abort |
| Usercopy | `platform/x64/usercopy.S` | optimized | optimized | optimized | optimized |
| Capability tokens | `platform/capabilities.rs` | runtime nonce | zero-cost | compiled away | compiled away |

## Implementation Phases

### Phase 0: Feature flag infrastructure ✓
Cargo features, `compile_error!` guard, Makefile `PROFILE` variable.

### Phase 1: Performance profile ✓
Concrete service types behind `cfg`. No vtable dispatch.

### Phase 2: Ludicrous profile ✓
Skip `access_ok()`, `#![allow(unsafe_code)]`.

### Phase 3: Optimized usercopy ✓
Alignment-aware `rep movsq` bulk copy in `platform/x64/usercopy.S`.

### Phase 4: Fortress copy-semantic frames ✓
`PageFrame` with `read()`/`write()`. `page_as_slice_mut` removed under Fortress.

### Phase 5: catch_unwind ✓
Dual target specs (`x64.json` abort, `x64-unwind.json` unwind). Dual linker
scripts (`.eh_frame` preserved for unwind). `unwinding` crate (v0.2) for
bare-metal unwinding. `call_service()` wrapper with `catch_unwind`.

### Phase 6: Capability tokens ✓
`Cap<T>` in `platform/capabilities.rs`. Fortress: runtime-validated nonce.
Balanced: zero-cost newtype. Performance/Ludicrous: compiled away.
`Cap<NetAccess>` minted at network stack registration.

### Phase 7: Benchmarks and CI
Syscall latency, usercopy throughput, page fault latency, tmpfs I/O.
CI matrix: 4 profiles × 2 architectures × debug/release.

## Comparison with Other Approaches

No other Linux-compatible kernel offers configurable safety profiles:

| Kernel | Safety model | Configurable? |
|--------|-------------|---------------|
| Linux | None (all C) | No |
| Asterinas | Framekernel (~14% TCB) | No |
| RedLeaf | Language domains | No |
| **Kevlar** | **Ringkernel (3-100% TCB)** | **Yes — 4 profiles** |

The key innovation: safety is not a binary choice between "safe kernel that's
slower" and "fast kernel that's unsafe." It's a dial that users turn based on
their threat model and performance requirements.
