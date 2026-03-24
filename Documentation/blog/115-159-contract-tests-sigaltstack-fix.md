# Blog 115: 159/159 contract tests — SA_ONSTACK signal delivery fix

**Date:** 2026-03-24
**Milestone:** M10 Alpine Linux

## Summary

All 159 Linux ABI contract tests now pass. The final holdout — `signals.sigaltstack_xfail`
— was a signal delivery bug where `rt_sigreturn` restored the wrong stack pointer
after an SA_ONSTACK signal handler returned. Also fixed: CLOCK_REALTIME now genuinely
passes (deterministic output), mprotect RW→RO COW fix, and new debugging infrastructure.

## The Bug

When a signal is delivered with `SA_ONSTACK`, the kernel:
1. Saves the interrupted register context to `signaled_frame_stack`
2. Switches RSP to the alternate signal stack (`frame.rsp = alt_top`)
3. Calls `setup_signal_stack` to write a signal context frame on the alt stack
4. Returns to userspace — handler executes on the alt stack
5. Handler returns via `__restore_rt` → `rt_sigreturn` syscall
6. `rt_sigreturn` reads the saved context from the alt stack, restores registers

**The bug was in step 3.** `setup_signal_stack` saved `frame.rsp` (which was
already switched to `alt_top`) into the signal context at offset +16. When
`rt_sigreturn` restored from the context, it got `alt_top` as RSP instead of the
original user stack pointer.

After sigreturn, the program resumed with RSP pointing to the top of the alt stack.
musl's `__restore_sigs` function (which runs after the signal handler) executed
`ret` — popping from uninitialized alt stack memory, which contained 0x0. The CPU
jumped to address 0x0 → SIGSEGV.

## Debugging Process

**Why `println!` didn't work:** The signal delivery path is called from the syscall
return path while kernel locks may be held. `println!` acquires the serial lock,
causing a deadlock. Every attempt to add `println!` to the signal path caused the
kernel to hang.

**Lock-free tracing:** Built `emergency_serial_hex()` in the platform crate — raw
`outb` to COM1 port 0x3F8, no locking, no allocation. Safe from any context:

```rust
// In platform/x64/serial.rs:
pub fn emergency_serial_hex(prefix: &[u8], value: u64) {
    for &ch in prefix { unsafe { outb(SERIAL0_IOPORT, ch); } }
    // ... emit "=0x" + 16 hex digits + newline
}
```

**What the traces revealed:**

```
SIG:handler=0x0000000000401169    ← handler address (correct)
SIG:rsp_set=0x0000000a00001c58    ← signal frame RSP (correct)
POST:rip=0x0000000000401169       ← frame.rip correct after setup
POST:rsp=0x0000000a00001c58       ← frame.rsp correct after setup
FINAL:rip=0x0000000000401169      ← FIRST syscall return: handler entered ✓
FINAL:rsp=0x0000000a00001c58
FINAL:rip=0x00000000004064e1      ← SECOND syscall return: __restore_sigs
FINAL:rsp=0x0000000a00002020      ← RSP is alt_top! Should be original stack!
SIGSEGV: ip=0x0, RSP=0xa00002028  ← __restore_sigs ret popped 0 from alt stack
```

The handler DID execute successfully (first FINAL pair). But after `rt_sigreturn`
restored the context, RSP was `0xa00002020` (alt stack top) instead of the original
user stack. `__restore_sigs` then called `ret`, popping from uninitialized memory.

## The Fix

Pass the **original RSP** (captured before the alt stack switch) to
`setup_signal_stack`, which saves it in the signal context instead of `frame.rsp`:

```rust
// In kernel/process/process.rs — signal delivery:
let original_rsp = { frame.rsp };           // BEFORE alt switch
// ... frame.rsp = alt_top; ...             // Alt stack switch
let result = setup_signal_stack(
    frame, signal, handler, restorer, mask,
    original_rsp,                            // NEW parameter
);

// In platform/x64/task.rs — setup_signal_stack:
let regs: [u64; 19] = [
    saved_sigmask,
    { frame.rip }, original_rsp, { frame.rbp },  // Save ORIGINAL rsp
    // ... other registers ...
];
```

Now `rt_sigreturn` reads the correct original RSP from the signal context, and
the program resumes on the correct stack.

## Other Fixes in This Session

### mprotect RW→RO (COW fix)

The page fault COW handler for MAP_PRIVATE was too broad — it COW'd ALL
MAP_PRIVATE pages on write-to-RO, including anonymous ones. This meant
`mprotect(PROT_READ)` on anonymous pages was ineffective. Fix: only trigger
COW for file-backed MAP_PRIVATE pages (`!is_anonymous`).

### CLOCK_REALTIME

The test was marked XFAIL because Linux and Kevlar outputs differed (different
`tv_sec` timestamps from sequential execution). Fixed by removing timestamps
from the success output. The RTC reads correctly — `tv_sec=1774352558`
(March 2026, Unix epoch).

### Enhanced Crash Dump

SIGSEGV null pointer handler now prints full register state (RAX-R15, RIP,
RFLAGS, fault_addr) using the crash_regs infrastructure. Previously only
printed pid, ip, and fsbase.

## Test Results

| Suite | Result |
|-------|--------|
| **Contract tests** | **159/159 PASS** |
| Ext4 functional | 29/29 PASS |
| BusyBox | 100/100 PASS |
| SMP threading | 14/14 PASS |

## Files Changed

- `platform/x64/task.rs` — `setup_signal_stack` takes `original_rsp` parameter
- `platform/arm64/task.rs` — matching signature change
- `kernel/process/process.rs` — capture original RSP before alt switch, pass to setup
- `kernel/mm/page_fault.rs` — COW fix for mprotect, enhanced crash dump
- `platform/x64/serial.rs` — `emergency_serial_hex()` lock-free debug output
- `platform/x64/mod.rs`, `platform/lib.rs` — export emergency_serial_hex
- `testing/contracts/time/clock_realtime.c` — deterministic output
- `tools/gdb-debug-signal.py` — automated GDB debugging tool
