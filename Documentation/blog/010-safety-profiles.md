# Configurable Safety: Choose Your Own Tradeoff

**Date:** 2026-03-08

---

Every Rust OS makes the same pitch: "safe by default." Asterinas confines unsafe to 14% of its codebase. RedLeaf isolates faults in language domains. Theseus builds everything in safe Rust. All of them pick a single point on the safety/performance spectrum and freeze it in place.

Kevlar doesn't pick one point. It gives you the dial.

## The problem with fixed safety

A kernel running a stock exchange needs every safety mechanism available — copy-semantic page frames, runtime capability validation, panic containment at service boundaries. It can afford 15-25% overhead.

A kernel running Wine for gaming needs every cycle. Bounds checking on hot paths, vtable dispatch through trait objects, catch_unwind overhead — none of it is worth the frame time cost.

Today you have to choose between "safe kernel that's slower" and "fast kernel in C." We think that's a false choice. The safety mechanisms are independent, composable, and their costs are measurable. Why not let the operator decide?

## Four profiles, one flag

```bash
make run PROFILE=fortress      # Maximum safety
make run PROFILE=balanced      # Default — the sweet spot
make run PROFILE=performance   # Monolithic speed, platform-only unsafe
make run PROFILE=ludicrous     # Everything off, beat Linux
```

Each profile is a set of Cargo features that control compile-time decisions. No runtime flags, no dynamic dispatch where it isn't wanted, no code that isn't needed.

### Fortress (-15-25% vs Linux, ~3% unsafe)

Every safety layer enabled. Three rings with `catch_unwind` — a panicking filesystem returns `EIO` instead of crashing the kernel. Page frames accessible only through copy operations (no `&mut [u8]` into physical memory). Runtime-validated capability tokens at service boundaries.

This is for environments where correctness matters more than throughput.

### Balanced (-5-10% vs Linux, ~10% unsafe)

The default. Three rings with catch_unwind for fault containment. Direct-mapped page frames (the standard approach). Compile-time capability tokens that vanish at optimization. Optimized usercopy.

This is the profile most people should use.

### Performance (~parity with Linux, ~10% unsafe)

Two rings. Services compile as concrete types — `SmoltcpNetworkStack` instead of `dyn NetworkStackService`. The compiler monomorphizes everything, inlines service calls, eliminates vtable dispatch. No catch_unwind overhead.

Same amount of unsafe code as Balanced. Same platform crate, same safe wrappers. The only thing you lose is fault containment — a service panic crashes the kernel instead of being caught. For most workloads, that tradeoff is worth it.

### Ludicrous (potentially faster than Linux, 100% unsafe)

Everything off. `#![allow(unsafe_code)]` everywhere. Skip `access_ok()` bounds checking on user pointers (rely on the page fault handler). `get_unchecked()` on proven-safe hot paths.

Rust still provides its baseline guarantees — ownership, lifetimes, type safety within safe code. This mode strips the *kernel-specific* safety layers, not Rust itself. The performance advantage over Linux comes from monomorphization, zero-cost abstractions, and better aliasing information for the optimizer.

## Why this is a single Cargo feature, not four separate kernels

Cargo's feature system is the perfect mechanism. Features are additive, resolved at compile time, and produce a single binary. The `platform/` crate owns the profile flags:

```toml
[features]
default = ["profile-balanced"]
profile-fortress = []
profile-balanced = []
profile-performance = []
profile-ludicrous = []
```

Higher crates forward features through Cargo's unification. A `compile_error!` guard ensures exactly one profile is active. The Makefile maps `PROFILE=` to `--features`.

Most of the kernel code is profile-independent. The `cfg` decision points are concentrated in a handful of files:

- `platform/address.rs` — `access_ok()` present or compiled out
- `platform/page_ops.rs` — `OwnedFrame` or `page_as_slice_mut`
- `kernel/services.rs` — `dyn Trait` or concrete type dispatch, catch_unwind wrapper
- `kernel/main.rs` — `deny(unsafe_code)` or `allow(unsafe_code)`
- Target spec JSON — `panic = "unwind"` or `panic = "abort"`

## The catch_unwind problem

There's one hard part: `catch_unwind` requires `panic = "unwind"`, but bare-metal kernels typically use `panic = "abort"` (smaller binaries, no unwinding tables). Fortress and Balanced need a separate target spec with `panic = "unwind"`, plus the `unwinding` crate for a no-std unwinder.

We're implementing this last, after the simpler profiles work. If it proves too complex for bare-metal, we'll use a fail-stop model where service panics are logged distinctly from core panics but still halt the kernel.

## The competitive picture

| Kernel | Safety model | Configurable? | TCB |
|--------|-------------|---------------|-----|
| Linux | None (C) | No | 100% |
| Asterinas | Framekernel | No | ~14% |
| RedLeaf | Language domains | No | varies |
| **Kevlar** | **Ringkernel** | **Yes — 4 profiles** | **3-100%** |

No other Linux-compatible kernel offers this. The idea is simple: safety mechanisms are compile-time decisions with measurable costs. Make them configurable. Let the operator choose.

## Implementation plan

We're building this bottom-up:

1. Feature flag plumbing (Cargo features, Makefile integration)
2. Performance profile (concrete service types, no vtable dispatch)
3. Ludicrous profile (skip access_ok, allow unsafe)
4. Optimized usercopy (alignment-aware rep movsq)
5. Fortress copy-semantic frames (OwnedFrame)
6. catch_unwind (unwind-capable target spec — highest risk)
7. Capability tokens
8. Benchmarks and CI matrix across all profiles

The goal: every profile boots BusyBox. Then we measure.
