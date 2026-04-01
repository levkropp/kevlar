# Blog 141: Overflow checks broke two profiles — and catch_unwind hid it

**Date:** 2026-04-01
**Milestone:** M6 SMP — Stability / CI parity

## Summary

The performance and ludicrous profiles crashed on boot with corrupted
panic messages while balanced and fortress worked fine. Root cause:
Rust's overflow-checked arithmetic in the dev profile caused panics in
syscall handlers. The balanced/fortress profiles masked these panics
via `catch_unwind` (returning -EIO to userspace), but performance and
ludicrous call syscall handlers directly — so the overflow panic
propagated to the kernel panic handler and crashed the system.

Fix: disable `overflow-checks` in the dev profile (matching the release
profile) and use `saturating_mul`/`saturating_add` for the ppoll timeout
calculation. All 4 profiles now pass 100/100 busybox-suite on SMP 4.

## The crash

```
[PANIC] CPU=0 at kernel/syscalls/mod.rs:1365
DBG {"type":"panic","message":" ticks/10mspllocate cpu_local for APIC ID...
```

Performance and ludicrous produced zero serial output via `run-qemu.py
--batch`. GDB revealed the kernel actually booted and halted cleanly —
but a panic during the first syscall corrupted the serial output stream
with garbled .rodata fragments.

The panic message was corrupted because the panic handler itself
triggered further exceptions (a BREAKPOINT from hitting `int3` padding
bytes after jumping to a wrong address in the corrupted panic path).

## Investigation

### Phase 1: Is it a boot issue?

- Zero serial output suggested early boot hang
- GDB attached to KVM showed CPU in `halt()` — the kernel had booted
  and shut down cleanly
- Under TCG (no KVM), ludicrous booted and ran to userspace normally
- This ruled out page table and memory layout issues

### Phase 2: Finding the actual output

Running with longer timeout captured the real output: the kernel booted,
started the busybox suite, then immediately panicked at
`kernel/syscalls/mod.rs:1365` — the ppoll timeout calculation.

### Phase 3: Why only two profiles?

The four profiles differ primarily in `call_service()`:

```rust
// Balanced / Fortress:
pub fn call_service<R>(f: impl FnOnce() -> Result<R>) -> Result<R> {
    match catch_unwind(f) {
        Ok(result) => result,
        Err(_) => Err(Errno::EIO),  // panic → -EIO
    }
}

// Performance / Ludicrous:
pub fn call_service<R>(f: impl FnOnce() -> Result<R>) -> Result<R> {
    f()  // panic → kernel crash
}
```

With `catch_unwind`, any panic inside a syscall handler is caught and
converted to `-EIO`. Without it, the panic propagates and crashes the
kernel.

### Phase 4: The overflow

```rust
// ppoll timeout: seconds × 1000 + nanoseconds / 1_000_000
let ms = ts[0] * 1000 + ts[1] / 1_000_000;  // OVERFLOW!
```

BusyBox's shell calls `ppoll()` with a large timeout. `ts[0] * 1000`
overflows i64 when seconds > 9.2 × 10^15. With overflow-checks enabled
(the default in Rust's dev profile), this triggers a checked-multiply
panic.

The workspace `Cargo.toml` had `debug-assertions = false` but did NOT
set `overflow-checks = false` in the dev profile. Overflow checks
default to `true` even when debug assertions are off.

## The fix

Two changes:

**1. Disable overflow-checks in dev profile** (`Cargo.toml`):
```toml
[profile.dev]
opt-level = 2
debug-assertions = false
overflow-checks = false   # ← added
panic = "abort"
codegen-units = 1
```

This matches the release profile and prevents overflow panics in ALL
arithmetic, not just the ppoll case. Overflow-checked arithmetic is
useful for application development but dangerous in a kernel where
`catch_unwind` isn't always available.

**2. Saturating arithmetic for ppoll timeout** (`kernel/syscalls/mod.rs`):
```rust
let ms = ts[0].saturating_mul(1000)
              .saturating_add(ts[1] / 1_000_000);
```

Belt-and-suspenders: even without overflow checks, use saturating
operations for user-supplied values that could be extreme.

## Results

| Profile | Before | After |
|---------|--------|-------|
| ludicrous | 0/100 (panic on first syscall) | 100/100 |
| performance | 0/100 (same) | 100/100 |
| balanced | 100/100 | 100/100 |
| fortress | 100/100 | 100/100 |

Binary sizes decreased ~60KB (48137398 → 48080054 for balanced) due to
removal of overflow check instrumentation.

## Lessons

1. **Overflow checks are a profile hazard in kernels.** If some code
   paths have `catch_unwind` and others don't, overflow panics will
   crash only the unprotected paths — making the bug invisible in the
   "safe" profiles.

2. **Corrupted panic messages = the panic handler itself is broken.**
   When the panic format string shows fragments of unrelated strings,
   the stack or .rodata addressing is corrupt. Don't trust the reported
   line number.

3. **"No serial output" ≠ "boot hang".** Always verify with GDB or
   QEMU monitor before assuming early boot failure. The kernel may have
   booted, crashed, and shut down so fast that no output reached the
   terminal.
