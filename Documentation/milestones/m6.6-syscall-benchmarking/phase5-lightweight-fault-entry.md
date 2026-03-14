# M6.6 Phase 5: Lightweight Page Fault Entry

**Duration:** ~1 day
**Prerequisite:** Phase 4
**Goal:** Reduce page fault exception handler overhead from ~150ns to ~70ns.

## Current overhead

Our page fault path (platform/x64/interrupt.rs + trap.S):

1. **trap.S**: push all 16 GPRs + error code → InterruptFrame (~30ns)
2. **x64_handle_interrupt**: match on vector, read CR2, construct
   PageFaultReason, call handler via function pointer (~20ns)
3. **handle_page_fault**: the actual work (~1600ns for 17 pages)
4. **x64_check_signal_on_irq_return**: construct PtRegs from
   InterruptFrame, call try_delivering_signal, write back modified
   regs (~50ns)
5. **trap.S**: pop all 16 GPRs, iretq (~30ns)

Steps 1, 2, 4, 5 cost ~130ns total.  Linux saves ~6 callee-saved
registers and skips signal check on the fast path.

## Optimizations

### 1. Save only callee-saved registers in trap.S

Currently we push all 16 GPRs.  The C ABI only requires preserving
rbx, rbp, r12-r15 (6 registers).  The page fault handler is a Rust
function — it follows the C ABI and will save/restore any caller-saved
registers it uses.

Change trap.S to only save the 6 callee-saved registers + rflags
for the page fault vector.  Other interrupt vectors (timer, IPI) can
keep the full save since they need the complete context for preemption.

**Savings:** 10 fewer push/pop pairs × ~1ns each = ~20ns per fault.

### 2. Skip signal check when no signals pending

`x64_check_signal_on_irq_return` constructs a full PtRegs (copying 18
fields from InterruptFrame) and calls `try_delivering_signal`.  The
fast path in try_delivering_signal checks `signal_pending.load() == 0`
and returns immediately, but the PtRegs construction still happens.

Optimization: check `signal_pending` BEFORE constructing PtRegs:

```rust
fn x64_check_signal_on_irq_return(frame: *mut InterruptFrame) {
    // Fast path: skip PtRegs construction if no signals pending
    if current_process().signal_pending.load(Ordering::Relaxed) == 0 {
        return;
    }
    // Slow path: construct PtRegs, deliver signal
    let mut pt = PtRegs { ... };
    handler().handle_interrupt_return(&mut pt);
    frame.rip = pt.rip;
    ...
}
```

**Savings:** ~30ns per fault (skip 18-field struct copy + function call).

### 3. Direct handler call (skip function pointer dispatch)

`x64_handle_interrupt` dispatches through a `handler()` function pointer
that returns a `&dyn KernelOps` trait object.  For the page fault vector,
we know exactly which function to call.  A direct call avoids the vtable
lookup.

**Savings:** ~5ns per fault (one indirect branch → direct call).

## Files to modify

- `platform/x64/trap.S` — page fault vector uses minimal register save
- `platform/x64/interrupt.rs` — fast-path signal check, direct handler call
- Build and verify: `make bench-kvm`, `make test-contracts-vm`

## Expected impact

Combined savings: ~55ns per fault.  With 256 faults for mmap_fault
benchmark: 256 × 55ns = ~14µs total.  Per page (÷4096): ~3.4ns/page.
Small per-page but meaningful for the total benchmark time.
