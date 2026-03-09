// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// ARM64 user-space entry/exit and context switch.
//
// TPIDR_EL1 offsets (CpuLocalHead):
.set TPIDR_SP_EL1, 0
.set TPIDR_SP_EL0_SAVE, 8

// PtRegs offsets:
// regs[0..30] at 0..240, sp at 248, pc at 256, pstate at 264

/// Enter user space for the first time (new thread).
/// The kernel stack has a PtRegs frame at the top.
.global userland_entry
userland_entry:
    // Sanitize all registers to prevent information leak.
    mov     x0, #0
    mov     x1, #0
    mov     x2, #0
    mov     x3, #0
    mov     x4, #0
    mov     x5, #0
    mov     x6, #0
    mov     x7, #0
    mov     x8, #0
    mov     x9, #0
    mov     x10, #0
    mov     x11, #0
    mov     x12, #0
    mov     x13, #0
    mov     x14, #0
    mov     x15, #0
    mov     x16, #0
    mov     x17, #0
    mov     x18, #0
    // x19-x28 are callee-saved, but zero them for the first entry.
    mov     x19, #0
    mov     x20, #0
    mov     x21, #0
    mov     x22, #0
    mov     x23, #0
    mov     x24, #0
    mov     x25, #0
    mov     x26, #0
    mov     x27, #0
    mov     x28, #0
    mov     x29, #0
    mov     x30, #0

    // Pop the saved PtRegs from the kernel stack to set up the ERET frame.
    // The PtRegs was pushed by new_user_thread (sp, pc, pstate).
    ldr     x0, [sp, #(31 * 8)]    // sp_el0
    msr     sp_el0, x0
    ldr     x0, [sp, #(32 * 8)]    // elr_el1 (user PC)
    msr     elr_el1, x0
    ldr     x0, [sp, #(33 * 8)]    // spsr_el1 (user PSTATE)
    msr     spsr_el1, x0
    // Clear x0 again.
    mov     x0, #0
    add     sp, sp, #(34 * 8)      // Clean up the frame

    // Enable interrupts in the user PSTATE via SPSR.
    // SPSR.I = 0 (IRQ unmasked) is already set by new_user_thread.
    eret

/// Entry point for a forked child process.
/// The kernel stack has: PtRegs frame (from fork).
/// We need to set return value (x0) to 0 and ERET to user.
.global forked_child_entry
forked_child_entry:
    // Restore all registers from the PtRegs on the stack.
    // But set x0 = 0 (fork return value for child).
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

    // Restore sp_el0, elr_el1, spsr_el1.
    ldr     x1, [sp, #(31 * 8)]
    msr     sp_el0, x1
    ldr     x1, [sp, #(32 * 8)]
    msr     elr_el1, x1
    ldr     x1, [sp, #(33 * 8)]
    msr     spsr_el1, x1

    add     sp, sp, #(34 * 8)

    // Set fork() return value = 0.
    mov     x0, #0
    mov     x1, #0

    eret

/// Entry point for a kernel thread.
.global kthread_entry
kthread_entry:
    // Enable interrupts.
    msr     daifclr, #2

    // Clear frame pointer.
    mov     x29, #0

    // Pop the entry point and call it.
    ldr     x0, [sp], #8
    blr     x0

    // Unreachable.
    brk     #0

/// Context switch between two threads.
/// fn do_switch_thread(prev_sp: *mut u64, next_sp: *const u64)
///   x0 = &prev.sp (save current SP here)
///   x1 = &next.sp (load new SP from here)
.global do_switch_thread
do_switch_thread:
    // Save callee-saved registers and LR.
    stp     x19, x20, [sp, #-16]!
    stp     x21, x22, [sp, #-16]!
    stp     x23, x24, [sp, #-16]!
    stp     x25, x26, [sp, #-16]!
    stp     x27, x28, [sp, #-16]!
    stp     x29, x30, [sp, #-16]!

    // Save NZCV flags.
    mrs     x9, nzcv
    str     x9, [sp, #-8]!

    // Save current SP to prev->sp.
    mov     x9, sp
    str     x9, [x0]

    // Load next SP.
    ldr     x9, [x1]
    mov     sp, x9

    // Restore NZCV flags.
    ldr     x9, [sp], #8
    msr     nzcv, x9

    // Restore callee-saved registers and LR.
    ldp     x29, x30, [sp], #16
    ldp     x27, x28, [sp], #16
    ldp     x25, x26, [sp], #16
    ldp     x23, x24, [sp], #16
    ldp     x21, x22, [sp], #16
    ldp     x19, x20, [sp], #16

    // Return to the next thread's saved LR (x30).
    ret
