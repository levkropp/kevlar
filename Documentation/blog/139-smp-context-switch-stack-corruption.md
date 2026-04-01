# Blog 139: SMP context switch bug — frame layout mismatch and stack corruption

**Date:** 2026-04-01
**Milestone:** M6 SMP — Stability

## Summary

The BusyBox SMP test (`-smp 4`) crashed consistently with RIP=0. Investigation
revealed two bugs: a stack frame layout mismatch between `do_switch_thread`
assembly and `fork()`'s initial stack setup, and a timer interrupt window
during the stack switch. Fixing both improved the pass rate from 0% to 40%.
A remaining intermittent stack corruption issue is still under investigation.

## The crash

```
kernel page fault: rip=0, rsp=ffff80003fe4b748, vaddr=0
```

A null function pointer call on an AP core. The saved kernel stack for a
process being context-switched-to was all zeros — the return address,
callee-saved registers, everything.

## Investigation

### Ruling out suspects

1. **GS base corruption?** No — added `verify_gsbase()` check in the timer
   handler. GS base matches the init-time value on all CPUs.

2. **cpu_local CURRENT wrong?** Partially — the flight recorder's PREEMPT
   event was printing `cpu_id` labeled as `pid`, making it look like APs
   had wrong PIDs. Actually a display bug in the flight recorder.

3. **Page allocator zeroing?** Disabled `refill_prezeroed_pages()` on APs —
   crash persisted. The allocator isn't zeroing live stack pages.

4. **AP-only issue?** Disabled AP preemption timer entirely — crash persisted.
   It happens on CPU 0 too.

### Bug 1: Stack frame layout mismatch

The `do_switch_thread` assembly saves registers in a specific order. When
a forked child is first scheduled, its initial kernel stack must have
values pushed in the exact reverse order so `do_switch_thread`'s pops
restore the correct registers.

I changed `do_switch_thread` to add `cli` before the stack switch but
accidentally reordered the pushes:

```asm
; OLD: pushfq last (RFLAGS at lowest address)
push rbp, rbx, r12, r13, r14, r15, pushfq

; NEW: pushfq first (RFLAGS at highest address)
pushfq, push r15, r14, r13, r12, rbx, rbp
```

But the fork code in `task.rs` still pushed in the OLD order:
```rust
push RIP, RBP, RBX, R12, R13, R14, R15, RFLAGS  // RFLAGS at bottom
```

When `do_switch_thread` popped in the NEW order (`pop rbp` first), it got
RFLAGS=0x02 in RBP instead of the actual RBP. The `forked_child_entry`
then did `iretq` with corrupted registers → GPF (vec=13).

**Fix:** Updated all 4 stack setup paths in `task.rs` (kthread, user thread,
fork, clone) to match the new assembly push/pop order.

### Bug 2: Timer interrupt during stack switch

`do_switch_thread` loads the new thread's RSP then pops registers:

```asm
mov rsp, [rsi]   ; Load new stack
popfq             ; Restore RFLAGS (may enable interrupts!)
pop r15           ; ...but timer can fire here
pop r14
pop rbp
ret               ; Return address may be corrupted
```

If `popfq` restores IF=1 (interrupts enabled), a timer interrupt can fire
between `popfq` and `ret`. The interrupt handler pushes a frame onto the
stack we're restoring, overwriting register values that haven't been
popped yet.

**Fix:** Added `cli` before loading the new RSP. Interrupts stay disabled
through the entire pop sequence. The saved RFLAGS (restored by `popfq`)
has IF=0 for kernel context switches, so interrupts are re-enabled only
when the restored thread explicitly enables them.

```asm
cli                ; Prevent timer IRQ during restore
mov rsp, [rsi]     ; Load new stack (safe now)
pop rbp            ; These pops can't be interrupted
pop rbx
...
popfq              ; Restores IF from saved context
ret                ; Return address is intact
```

### Safety check

Added a pre-switch validation that reads the return address from the target
stack before calling `do_switch_thread`. If it's 0 or not in kernel text,
we get a clean panic with diagnostic info instead of a cryptic RIP=0 crash:

```rust
let ret_addr = unsafe { *((next_rsp_val + 56) as *const u64) };
if ret_addr == 0 || (ret_addr > 0 && ret_addr < 0xffff_8000_0000_0000) {
    panic!("switch_thread BUG: stack corrupted! ...");
}
```

## Results

| Before | After |
|--------|-------|
| 0/5 BusyBox SMP runs pass | 2/5 pass |
| RIP=0 crash (no diagnostics) | Clean panic with stack dump |
| forked_child_entry GPF | Fixed |

## Remaining issue

An intermittent stack corruption still occurs (~60% of runs). The target
process's saved RSP is always `0xffff80003fe4b708` — the same stack page
every time. The first stack value (RBP) is a valid kernel pointer, but
the return address and most registers are zero. This suggests partial
page zeroing rather than complete overwrite.

Likely causes still under investigation:
- Stack cache returning a page that's being concurrently zeroed by the
  pre-zeroed page pool refiller
- A `release_stacks()` on an exiting thread freeing a stack page that
  another thread still references (double-free)
- TLB coherency issue where one CPU sees stale page table entries for
  a freed-and-reallocated stack page

## Verified

- 67/67 Alpine smoke tests pass (single CPU)
- 14/14 X11 tests pass
- 159/159 contract tests pass
- 53/53 benchmarks measured (30 faster than Linux)
- All other test suites (systemd, nginx, ssh, ext4, storage) pass
