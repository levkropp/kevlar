// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// ARM64 user-space entry/exit and context switch.
//
// The kernel is compiled -neon,-fp-armv8 so the Rust compiler never emits
// FP/NEON instructions.  The v-reg saves/loads in `do_switch_thread` and
// `kevlar_save_fp_to` below need the assembler to accept q-regs and
// FPCR/FPSR anyway, so re-enable FP+SIMD at the asm level.
.arch armv8-a+fp+simd

// TPIDR_EL1 offsets (CpuLocalHead):
.set TPIDR_SP_EL1, 0
.set TPIDR_SP_EL0_SAVE, 8

// PtRegs offsets:
// regs[0..30] at 0..240, sp at 248, pc at 256, pstate at 264

/// Enter user space for the first time (new thread).
/// The kernel stack has a PtRegs frame at the top.
.global userland_entry
userland_entry:
    // Re-enable preemption (switch() called preempt_disable before
    // do_switch_thread, but this entry bypasses switch()'s return path).
    mrs     x0, tpidr_el1
    ldr     w1, [x0, #16]
    sub     w1, w1, #1
    str     w1, [x0, #16]

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
    // Re-enable preemption (switch() called preempt_disable before
    // do_switch_thread, but this entry bypasses switch()'s return path).
    mrs     x0, tpidr_el1
    ldr     w1, [x0, #16]
    sub     w1, w1, #1
    str     w1, [x0, #16]

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
/// fn do_switch_thread(
///     prev_sp: *mut u64,        // x0
///     next_sp: *const u64,      // x1
///     ctx_saved: *mut u8,       // x2
///     prev_fp: *mut FpState,    // x3  (save outgoing v0-v31 / FPCR / FPSR here)
///     next_fp: *const FpState,  // x4  (load incoming v0-v31 / FPCR / FPSR from here)
/// )
///
/// The store-release to *ctx_saved (via stlrb) ensures that any CPU that
/// observed context_saved=0 and is spinning in resume() will see the fully
/// saved SP before being allowed to enqueue this thread.
///
/// FP save/restore: the kernel is compiled -neon,-fp-armv8, so EL1 code
/// never touches v-regs.  User FP state therefore survives EL0→EL1→EL0
/// round-trips untouched and only needs to be saved when we actually swap
/// tasks.  Matches Linux arm64's fpsimd_save_state / fpsimd_load_state.
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

    // Save outgoing FP/NEON state into *prev_fp (x3).  Placed before the
    // SP save so the full context is captured while other CPUs are still
    // gated by context_saved=0.
    stp     q0,  q1,  [x3, #(0  * 32)]
    stp     q2,  q3,  [x3, #(1  * 32)]
    stp     q4,  q5,  [x3, #(2  * 32)]
    stp     q6,  q7,  [x3, #(3  * 32)]
    stp     q8,  q9,  [x3, #(4  * 32)]
    stp     q10, q11, [x3, #(5  * 32)]
    stp     q12, q13, [x3, #(6  * 32)]
    stp     q14, q15, [x3, #(7  * 32)]
    stp     q16, q17, [x3, #(8  * 32)]
    stp     q18, q19, [x3, #(9  * 32)]
    stp     q20, q21, [x3, #(10 * 32)]
    stp     q22, q23, [x3, #(11 * 32)]
    stp     q24, q25, [x3, #(12 * 32)]
    stp     q26, q27, [x3, #(13 * 32)]
    stp     q28, q29, [x3, #(14 * 32)]
    stp     q30, q31, [x3, #(15 * 32)]
    mrs     x9,  fpcr
    mrs     x10, fpsr
    str     x9,  [x3, #512]    // FPCR
    str     x10, [x3, #520]    // FPSR

    // Save current SP to prev->sp, then mark context as saved.
    mov     x9, sp
    str     x9, [x0]
    mov     w9, #1
    stlrb   w9, [x2]        // store-release: context_saved = true

    // Load next SP.
    ldr     x9, [x1]
    mov     sp, x9

    // Load incoming FP/NEON state from *next_fp (x4).  Placed after the SP
    // swap so first-run tasks (userland_entry / forked_child_entry / kthread_entry)
    // eret with the correct v-regs.
    ldr     x9,  [x4, #512]    // FPCR
    ldr     x10, [x4, #520]    // FPSR
    msr     fpcr, x9
    msr     fpsr, x10
    ldp     q0,  q1,  [x4, #(0  * 32)]
    ldp     q2,  q3,  [x4, #(1  * 32)]
    ldp     q4,  q5,  [x4, #(2  * 32)]
    ldp     q6,  q7,  [x4, #(3  * 32)]
    ldp     q8,  q9,  [x4, #(4  * 32)]
    ldp     q10, q11, [x4, #(5  * 32)]
    ldp     q12, q13, [x4, #(6  * 32)]
    ldp     q14, q15, [x4, #(7  * 32)]
    ldp     q16, q17, [x4, #(8  * 32)]
    ldp     q18, q19, [x4, #(9  * 32)]
    ldp     q20, q21, [x4, #(10 * 32)]
    ldp     q22, q23, [x4, #(11 * 32)]
    ldp     q24, q25, [x4, #(12 * 32)]
    ldp     q26, q27, [x4, #(13 * 32)]
    ldp     q28, q29, [x4, #(14 * 32)]
    ldp     q30, q31, [x4, #(15 * 32)]

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

/// Snapshot live HW FP/NEON state into *dst.
/// fn kevlar_save_fp_to(dst: *mut FpState)  // x0 = dst
///
/// Used by `ArchTask::fork` / `new_thread` to copy the parent's v-regs into
/// the child's FpState at clone/fork time — needed because the trap path no
/// longer saves FP state, so the parent's v-regs are still live in HW when
/// the SVC handler runs.
.global kevlar_save_fp_to
kevlar_save_fp_to:
    stp     q0,  q1,  [x0, #(0  * 32)]
    stp     q2,  q3,  [x0, #(1  * 32)]
    stp     q4,  q5,  [x0, #(2  * 32)]
    stp     q6,  q7,  [x0, #(3  * 32)]
    stp     q8,  q9,  [x0, #(4  * 32)]
    stp     q10, q11, [x0, #(5  * 32)]
    stp     q12, q13, [x0, #(6  * 32)]
    stp     q14, q15, [x0, #(7  * 32)]
    stp     q16, q17, [x0, #(8  * 32)]
    stp     q18, q19, [x0, #(9  * 32)]
    stp     q20, q21, [x0, #(10 * 32)]
    stp     q22, q23, [x0, #(11 * 32)]
    stp     q24, q25, [x0, #(12 * 32)]
    stp     q26, q27, [x0, #(13 * 32)]
    stp     q28, q29, [x0, #(14 * 32)]
    stp     q30, q31, [x0, #(15 * 32)]
    mrs     x1, fpcr
    mrs     x2, fpsr
    str     x1, [x0, #512]    // FPCR
    str     x2, [x0, #520]    // FPSR
    ret
