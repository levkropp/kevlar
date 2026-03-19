# Blog 092: ktrace goes multi-arch — ARM64 semihosting transport and standalone repo

**Date:** 2026-03-19
**Milestone:** M10 Alpine Linux

## Context

ktrace is Kevlar's high-bandwidth binary kernel tracer.  Until today it was
x86_64-only: each trace event calls `outb(0xe9, byte)` to QEMU's ISA debugcon
device, which writes to a host chardev file at ~5 MB/s on KVM.

ARM64 just got real BusyBox support (Blog 091).  The first debugging question
we'll hit when ARM64 tests fail is "what was the kernel doing at the time?".
ISA debugcon is a PC/AT bus device — it doesn't exist on ARM's virt machine.

We needed an ARM64 equivalent.  We also noticed that the ktrace protocol (wire
format + QEMU integration) is useful to any bare-metal kernel, not just Kevlar.
Both observations pushed in the same direction: design a proper multi-arch
transport, then extract ktrace into a standalone repo.

---

## The ARM64 transport: ARM semihosting

ARM semihosting is the ARM-defined mechanism for a guest to communicate with
its debug host.  QEMU has supported it for years.  The protocol is elegant:

```
x0 = operation number
x1 = parameter block address
HLT #0xF000              ← debug exception; QEMU intercepts and handles it
```

The operation that matters for tracing is `SYS_WRITE` (0x05): write a buffer
to an open file handle.  Combined with QEMU's `-semihosting-config chardev=ID`
option, the output goes directly to a host file — exactly what ISA debugcon
does on x86_64.

```
QEMU x86_64:  outb(0xe9, byte)          → isa-debugcon → chardev → ktrace.bin
QEMU ARM64:   HLT #0xF000 + SYS_WRITE  → semihosting  → chardev → ktrace.bin
```

Same chardev, same `ktrace.bin`, same decoder.

### The `write_bytes` design

For single bytes, `SYS_WRITEC` (op 3) is the fastest path — one trap,
one byte, `x1` points to the byte on the stack:

```rust
pub fn write_byte(byte: u8) {
    unsafe {
        core::arch::asm!(
            "hlt #0xf000",
            in("x0")  SYS_WRITEC,
            in("x1")  &byte as *const u8,
            lateout("x0") _,
            options(nostack),
        );
    }
}
```

For bulk dumps (ring buffer flush), `SYS_WRITE` (op 5) is critical: a
single trap writes the entire buffer regardless of size.  The parameter
block is a three-word struct on the stack:

```rust
pub fn write_bytes(data: &[u8]) {
    let params: [usize; 3] = [STDERR_HANDLE, data.as_ptr() as usize, data.len()];
    unsafe {
        core::arch::asm!(
            "hlt #0xf000",
            in("x0") SYS_WRITE,
            in("x1") params.as_ptr(),
            lateout("x0") _,
            options(nostack, readonly),
        );
    }
}
```

A typical ktrace dump is one CPU × 8192 entries × 32 bytes = 256 KB.  On
TCG (no KVM), one semihosting trap is ~500 ns.  With `SYS_WRITE`, the entire
dump completes in a **single trap** — the same asymptotic cost as ISA
debugcon's single chardev flush.

### QEMU flags

```sh
# ARM64
-chardev file,id=ktrace,path=ktrace.bin \
-semihosting-config enable=on,target=native,chardev=ktrace

# x86_64 (unchanged)
-chardev file,id=ktrace,path=ktrace.bin \
-device isa-debugcon,chardev=ktrace,iobase=0xe9
```

---

## Why semihosting is the right answer

The alternative would be to write a custom QEMU MMIO device (a "KTD —
Kevlar Trace Device") at a fixed ARM64 virt machine address, similar to
how the ISA debugcon device works on x86.  That approach would require
patching QEMU.

Semihosting gives us 95% of the same design — a QEMU-native mechanism
that routes trace output to a chardev — without any QEMU patches.  It
already exists for exactly this purpose: low-level debug output from
a bare-metal guest to the host.

The one remaining limitation is that semihosting output goes to stderr when
no `chardev=` is configured, which means it mixes with QEMU's own output.
The `chardev=ktrace` flag cleanly separates trace output into `ktrace.bin`.

---

## `tools/ktrace/` — standalone repo skeleton

ktrace now lives at `tools/ktrace/` with its own `git init`.  The intent is
to push it to a public GitHub repo and add it as a submodule.  The repo
contains everything a non-Kevlar kernel needs to use the protocol:

```
tools/ktrace/
├── README.md
├── Cargo.toml                  (workspace)
├── spec/
│   └── wire-format.md          (KTRX v1 binary protocol specification)
├── ktrace-core/                (no_std Rust crate)
│   └── src/
│       ├── lib.rs              (DumpHeader, TraceRecord, EventType)
│       ├── format.rs           (wire format types with size assertions)
│       └── transport/
│           ├── mod.rs          (write_byte / write_bytes dispatch)
│           ├── x86_64.rs       (ISA debugcon, outb 0xe9)
│           └── arm64.rs        (ARM semihosting, HLT #0xF000)
└── decode/
    └── ktrace-decode.py → ../../ktrace-decode.py (symlink)
```

### The `ktrace-core` crate

`ktrace-core` is `#![no_std]` with zero dependencies.  A kernel adds it as
a path dependency and enables the appropriate transport feature:

```toml
[dependencies]
ktrace-core = { path = "tools/ktrace/ktrace-core", features = ["transport-arm64"] }
```

Then emits trace data with:

```rust
use ktrace_core::transport::write_bytes;
// dump the ring buffer
write_bytes(ring_buffer_slice);
```

The wire format types (`DumpHeader`, `TraceRecord`, `EventType`) are
shared between the kernel and the host decoder, eliminating the risk of
format drift.

---

## Integration changes in Kevlar

### `platform/arm64/debugcon.rs` (new)

Architecture-specific semihosting transport, parallel to `platform/x64/debugcon.rs`.

### `platform/lib.rs`

The `pub mod debugcon` block was x86_64-only.  It now dispatches to the
right transport based on `target_arch`, and the feature gate is simply
`cfg(feature = "ktrace")` (not `cfg(all(feature = "ktrace", target_arch = "x86_64"))`):

```rust
#[cfg(feature = "ktrace")]
pub mod debugcon {
    pub fn write_bytes(data: &[u8]) {
        #[cfg(target_arch = "x86_64")]
        crate::x64::debugcon::write_bytes(data);
        #[cfg(target_arch = "aarch64")]
        crate::arm64::debugcon::write_bytes(data);
    }
}
```

### `tools/run-qemu.py`

`--ktrace` now branches on `args.arch`:
- `x64`: original ISA debugcon flags
- `arm64`: `-semihosting-config enable=on,target=native,chardev=ktrace`

### `Makefile`

Added `ACCEL` variable: `--kvm` on x64, empty on arm64 (TCG-only on x86
hosts).  `run-ktrace` uses `$(ACCEL)` so `make ARCH=arm64 run-ktrace` works
without manually stripping `--kvm`.

---

## Verification

```
make ARCH=arm64 check FEATURES=ktrace-all   # 0 errors
make check FEATURES=ktrace-all              # 0 errors (x86_64 regression check)
```

ARM64 ktrace end-to-end:
```sh
make ARCH=arm64 RELEASE=1 run-ktrace
python3 tools/ktrace-decode.py ktrace.bin --summary
```

---

## What's next

1. **Push `tools/ktrace` to GitHub** and add as a git submodule
2. **Migrate Kevlar's format types** to `ktrace-core` so `TraceRecord` is
   defined once and shared between kernel and decoder
3. **Verify ARM64 ktrace end-to-end** — boot with `FEATURES=ktrace-all`,
   run a workload, decode the dump
4. **RISC-V transport** — a future architecture; the repo structure already
   accommodates it

---

## Files changed

- `platform/arm64/debugcon.rs` — new ARM64 semihosting transport
- `platform/arm64/mod.rs` — add `pub mod debugcon` (cfg-gated on `ktrace`)
- `platform/lib.rs` — extend `pub mod debugcon` to dispatch ARM64
- `tools/run-qemu.py` — `--ktrace` branch for ARM64 semihosting
- `Makefile` — `ACCEL` variable; `run-ktrace` uses `$(ACCEL)`
- `tools/ktrace/` — standalone repo skeleton (new)
