# Blog 110: OpenRC boots clean — signal stack frame corruption fixed

**Date:** 2026-03-22
**Milestone:** M10 Alpine Linux

## The Fix

Alpine's OpenRC now boots with **zero crashes**:

```
   OpenRC 0.55.1 is starting up Linux 6.19.8 (x86_64)

 * Caching service dependencies ... [ ok ]
/ #
```

The crash that plagued every boot ("Caching service dependencies"
→ SIGSEGV) is completely eliminated.

## Root Cause

**Signal delivery corrupted the user stack.** Our signal stack setup
only reserved 128 bytes (red zone) + 8 bytes (return address) before
calling the handler:

```
interrupted RSP → [local variables]
                   [128 bytes red zone]
handler RSP →     [8 bytes return addr]
                   [handler's stack frame ← OVERLAPS ABOVE!]
```

When SIGCHLD was delivered during OpenRC's `rc_deptree_update()` (which
spawns init script parsers via `posix_spawn`), the signal handler's
stack frame overwrote a pointer in the parent function. The corrupted
pointer (`0x1e` = struct field offset from NULL) was passed to musl's
`__secs_to_zone`, which crashed writing to address `0x1e`.

## Investigation Trail

1. **addr2line with musl-dbg** confirmed crash in `__secs_to_zone`
   at `__tz.c:416` — stores `*zonename = __tzname[1]` where
   `zonename = 0x1e` (invalid output pointer)

2. **Standalone `rc_deptree_update()` test** reproduced the crash
   deterministically with a single librc API call

3. **Signal delivery analysis** revealed the handler's stack directly
   overlapped the interrupted function's locals — no signal frame
   (ucontext_t/siginfo_t) was reserved

## The Fix

Reserve 832 bytes (matching Linux's `struct rt_sigframe`) for the
signal frame before calling the handler. Also align RSP to 16 bytes
per x86_64 ABI:

```rust
// Red zone (128 bytes below RSP that the function may use)
user_rsp = user_rsp.sub(128);

// Signal frame reservation (ucontext_t + siginfo_t ≈ 832 bytes)
user_rsp = user_rsp.sub(832);

// 16-byte alignment
let aligned = user_rsp.value() & !0xF;
```

## Status

| Feature | Status |
|---------|--------|
| OpenRC boot | **Zero crashes** ✓ |
| GCC compile | **3/3 tests pass** ✓ |
| GCC execute | **"Hello from Kevlar!"** ✓ |
| Alpine shell | **Interactive `/ #`** ✓ |
| Signal delivery | **Stack-safe** ✓ |
| System time | **Correct (UTC)** ✓ |
