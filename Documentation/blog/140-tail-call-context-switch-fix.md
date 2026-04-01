# Blog 140: The tail call that destroyed every context switch

**Date:** 2026-04-01
**Milestone:** M6 SMP — Stability

## Summary

A 100%-reproducible crash during SMP context switches turned out to be caused
by the Rust compiler tail-call-optimizing the `do_switch_thread()` call.  The
compiler emitted `jmp` instead of `call`, tearing down `switch_task`'s stack
frame before the assembly had a chance to save it.  Fix: a one-line inline asm
barrier.  Three bugs total were fixed in the context switch path, bringing
the SMP busybox suite from 0% to 100% pass rate.

## The crash

```
switch_thread BUG: cpu=0 rsp_pa=0x3ff6b708 ret=0x0 canary=0x0
stack[0]=0xffff80003ff6b7e8  (RBP — valid kernel pointer)
stack[1..7]=0x0 0x0 0x0 0x0 0x0 0x0 0x0  (all zeros)
```

PID 1's saved kernel context had a valid RBP but every other register —
including RFLAGS and the return address — was zero.  RFLAGS=0 is impossible
on x86 (bit 1 is architecturally reserved as 1).  The crash was 100%
reproducible, even on a single CPU (`-smp 1`).

## Investigation

### Phase 1: Ruling out page corruption

Initial hypothesis: something was zeroing PID 1's syscall stack page between
context switches.

**Page watchpoint:** Added atomic checks to `alloc_page`, `free_pages`,
`zero_page`, and `refill_prezeroed_pages` that would panic if any operation
touched PID 1's stack page.  Result: the watchpoint never triggered, but the
bug disappeared.  Removing the checks brought the crash back.  Classic
Heisenbug.

**Release stacks guard:** Added a check that PID 1's stacks were never freed
via `release_stacks()`.  Never triggered.

**Prezeroed pool:** Disabled `refill_prezeroed_pages()` entirely.  Crash
persisted.

**SMP vs single CPU:** Crash reproduced on `-smp 1`.  Not a cross-CPU race.

### Phase 2: Assembly read-back

Added an immediate read-back check in `do_switch_thread` assembly, right after
the save:

```asm
mov [rdi], rsp           ; save RSP
mov byte ptr [rdx], 1    ; context_saved = true
; Verify the save actually wrote correctly
cmp qword ptr [rsp + 56], 0    ; return address must be non-zero
je .bad
test qword ptr [rsp + 48], 2   ; RFLAGS bit 1 must be set
jnz .ok
.bad: hlt
.ok:
```

Result: the check **passed** — values were correct at save time.  The
corruption happened later.

### Phase 3: Disassembly

Disassembled `switch_task` and found the smoking gun:

```asm
; switch_task epilogue — BEFORE calling do_switch_thread:
add    $0x248,%rsp      ; deallocate locals
pop    %rbx             ; restore callee-saved regs
pop    %r12
pop    %r13
pop    %r14
pop    %r15
pop    %rbp
jmp    do_switch_thread  ; TAIL CALL — no return address pushed!
```

The compiler destroyed `switch_task`'s entire stack frame, then jumped to
`do_switch_thread`.  This is a legal optimization for a normal function — but
`do_switch_thread` is a context switch.  It saves the current thread's state
and loads a different thread.  The "return" from `do_switch_thread` happens
in a completely different thread's context, which expects `switch_task`'s
frame to still be on the stack.

## Why the Heisenbug

Any code change that altered `switch_task`'s structure could tip the compiler
from `jmp` (tail call) to `call` (normal call):

| Change | Tail call? | Bug? |
|--------|-----------|------|
| No debug code | `jmp` | **CRASH** |
| Page watchpoint atomics | `call` | Pass |
| 2-page xsave allocation | `call` | Pass |
| `black_box(())` after call | `call` | Pass |

The additional code prevented the tail call by making the compiler think there
was work to do after `do_switch_thread` returned.

## The fix

```rust
unsafe {
    do_switch_thread(
        prev.rsp.get(),
        next.rsp.get(),
        prev.context_saved.as_ptr() as *mut u8,
    );
    // CRITICAL: prevent the compiler from tail-calling do_switch_thread.
    // do_switch_thread is a context switch — it returns in a DIFFERENT
    // thread's context.  A tail call tears down this frame first, but
    // the returning thread needs this frame intact to clean up correctly.
    core::arch::asm!("", options(nomem, nostack));
}
```

The empty inline asm block generates no instructions but acts as a compiler
barrier — the compiler must assume it has side effects and cannot move or
eliminate the `call`.

## All three context switch fixes

This session resolved the remaining ~60% crash rate from Blog 139.  Three
bugs were fixed in total:

### Bug 1: Stack frame layout mismatch (Blog 139)

`do_switch_thread`'s push order (`pushfq` first → RFLAGS at highest address)
didn't match `fork()`/`new_user_thread()`/`new_thread()`'s initial stack
setup.  Forked children started with registers in wrong slots.

**Fix:** Updated all 4 stack setup paths in `task.rs` to match the assembly:
```
push order:  pushfq, r15, r14, r13, r12, rbx, rbp
pop order:   rbp, rbx, r12, r13, r14, r15, popfq
```

### Bug 2: Timer interrupt during restore (Blog 139)

No `cli` before loading the next thread's RSP.  A timer interrupt between
`popfq` (which could re-enable IF) and `ret` would push a frame onto the
stack being restored.

**Fix:** Added `cli` before `mov rsp, [rsi]` in `do_switch_thread`.

### Bug 3: Tail call optimization (this blog)

The compiler tail-called `do_switch_thread`, destroying the caller's frame
before the context switch could save it.

**Fix:** Empty `asm!("")` barrier after the call.

## Results

| Config | Before | After |
|--------|--------|-------|
| `-smp 1` | 0% (100% crash) | 100% (5/5) |
| `-smp 4` | 0% (100% crash) | 100% (10/10) |
| busybox-suite | 0/100 tests | 100/100 tests |

## Lessons

1. **Context switch functions must not be tail-called.** Any function that
   saves state for one thread and returns in another thread's context needs
   its caller's frame intact.  Rust (and C) compilers don't know about context
   switches — they'll happily optimize the call away.

2. **Heisenbugs that vanish with debug code are often compiler optimizations.**
   If adding an atomic load or allocation change fixes a crash, check the
   disassembly — the compiler may be generating fundamentally different code.

3. **RFLAGS=0 is a strong signal.** On x86, bit 1 of RFLAGS is always 1.
   If you see RFLAGS=0 in a saved context, the save itself is broken — the
   value was never produced by `pushfq`.
