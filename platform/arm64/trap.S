// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// ARM64 exception vector table and trap handlers.
//
// The vector table must be 2KB-aligned, with each entry 128 bytes (32 insns).
// Since SAVE_REGS/RESTORE_REGS exceed 32 instructions, each vector entry
// branches to an out-of-line handler.

.macro SAVE_REGS
    // Save all general-purpose registers + special regs onto the stack.
    // Layout matches PtRegs: x0-x30, sp_el0, elr_el1, spsr_el1
    sub     sp, sp, #(34 * 8)
    stp     x0,  x1,  [sp, #(0  * 8)]
    stp     x2,  x3,  [sp, #(2  * 8)]
    stp     x4,  x5,  [sp, #(4  * 8)]
    stp     x6,  x7,  [sp, #(6  * 8)]
    stp     x8,  x9,  [sp, #(8  * 8)]
    stp     x10, x11, [sp, #(10 * 8)]
    stp     x12, x13, [sp, #(12 * 8)]
    stp     x14, x15, [sp, #(14 * 8)]
    stp     x16, x17, [sp, #(16 * 8)]
    stp     x18, x19, [sp, #(18 * 8)]
    stp     x20, x21, [sp, #(20 * 8)]
    stp     x22, x23, [sp, #(22 * 8)]
    stp     x24, x25, [sp, #(24 * 8)]
    stp     x26, x27, [sp, #(26 * 8)]
    stp     x28, x29, [sp, #(28 * 8)]
    str     x30,       [sp, #(30 * 8)]
    mrs     x0, sp_el0
    str     x0,        [sp, #(31 * 8)]
    mrs     x0, elr_el1
    str     x0,        [sp, #(32 * 8)]
    mrs     x0, spsr_el1
    str     x0,        [sp, #(33 * 8)]
.endm

.macro RESTORE_REGS
    // Restore spsr and elr first.
    ldr     x0,        [sp, #(33 * 8)]
    msr     spsr_el1, x0
    ldr     x0,        [sp, #(32 * 8)]
    msr     elr_el1, x0
    ldr     x0,        [sp, #(31 * 8)]
    msr     sp_el0, x0

    ldp     x0,  x1,  [sp, #(0  * 8)]
    ldp     x2,  x3,  [sp, #(2  * 8)]
    ldp     x4,  x5,  [sp, #(4  * 8)]
    ldp     x6,  x7,  [sp, #(6  * 8)]
    ldp     x8,  x9,  [sp, #(8  * 8)]
    ldp     x10, x11, [sp, #(10 * 8)]
    ldp     x12, x13, [sp, #(12 * 8)]
    ldp     x14, x15, [sp, #(14 * 8)]
    ldp     x16, x17, [sp, #(16 * 8)]
    ldp     x18, x19, [sp, #(18 * 8)]
    ldp     x20, x21, [sp, #(20 * 8)]
    ldp     x22, x23, [sp, #(22 * 8)]
    ldp     x24, x25, [sp, #(24 * 8)]
    ldp     x26, x27, [sp, #(26 * 8)]
    ldp     x28, x29, [sp, #(28 * 8)]
    ldr     x30,       [sp, #(30 * 8)]
    add     sp, sp, #(34 * 8)
.endm

// Each vector entry is 128 bytes = 32 instructions max.
// We only use a branch instruction in each entry.
.macro VECTOR_ENTRY label
    .balign 128
\label:
.endm

// ---------------------------------------------------------------
// Exception Vector Table
// ---------------------------------------------------------------
.balign 2048
.global exception_vectors
exception_vectors:

    // --- Current EL with SP_EL0 (not used — we always use SP_EL1) ---
    VECTOR_ENTRY vec_curr_sp0_sync
    b       unhandled_exception

    VECTOR_ENTRY vec_curr_sp0_irq
    b       unhandled_exception

    VECTOR_ENTRY vec_curr_sp0_fiq
    b       unhandled_exception

    VECTOR_ENTRY vec_curr_sp0_serror
    b       unhandled_exception

    // --- Current EL with SP_ELx (kernel exceptions) ---
    VECTOR_ENTRY vec_curr_spx_sync
    b       handle_curr_spx_sync

    VECTOR_ENTRY vec_curr_spx_irq
    b       handle_curr_spx_irq

    VECTOR_ENTRY vec_curr_spx_fiq
    b       unhandled_exception

    VECTOR_ENTRY vec_curr_spx_serror
    b       unhandled_exception

    // --- Lower EL using AArch64 (user-space exceptions) ---
    VECTOR_ENTRY vec_lower_a64_sync
    b       handle_lower_a64_sync

    VECTOR_ENTRY vec_lower_a64_irq
    b       handle_lower_a64_irq

    VECTOR_ENTRY vec_lower_a64_fiq
    b       unhandled_exception

    VECTOR_ENTRY vec_lower_a64_serror
    b       unhandled_exception

    // --- Lower EL using AArch32 (not supported) ---
    VECTOR_ENTRY vec_lower_a32_sync
    b       unhandled_exception

    VECTOR_ENTRY vec_lower_a32_irq
    b       unhandled_exception

    VECTOR_ENTRY vec_lower_a32_fiq
    b       unhandled_exception

    VECTOR_ENTRY vec_lower_a32_serror
    b       unhandled_exception

// ---------------------------------------------------------------
// Out-of-line exception handlers
// ---------------------------------------------------------------

handle_curr_spx_sync:
    SAVE_REGS
    mov     x0, #0          // from_user = false
    mov     x1, sp           // frame pointer
    bl      arm64_handle_exception
    RESTORE_REGS
    eret

handle_curr_spx_irq:
    SAVE_REGS
    mov     x0, #2          // type = IRQ, from kernel
    mov     x1, sp
    bl      arm64_handle_irq
    RESTORE_REGS
    eret

handle_lower_a64_sync:
    SAVE_REGS
    mov     x0, #1          // from_user = true
    mov     x1, sp           // frame pointer
    bl      arm64_handle_exception
    // Deliver any pending signal before returning to user (mirrors x86_64
    // x64_check_signal_on_irq_return).  This handles SIGSEGV from page
    // faults and other signals raised inside the exception handler.
    mov     x0, sp
    bl      arm64_check_signal_on_return
    RESTORE_REGS
    eret

handle_lower_a64_irq:
    SAVE_REGS
    mov     x0, #3          // type = IRQ, from user
    mov     x1, sp
    bl      arm64_handle_irq
    // Deliver any signal pending after the IRQ (e.g. SIGALRM from timer).
    mov     x0, sp
    bl      arm64_check_signal_on_return
    RESTORE_REGS
    eret

unhandled_exception:
    mrs     x0, esr_el1
    mrs     x1, elr_el1
    mrs     x2, far_el1
    bl      arm64_unhandled_exception
    b       .
