// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// ARM64 exception vector table and trap handlers.
//
// The vector table must be 2KB-aligned, with each entry 128 bytes (32 insns).
// Since SAVE_REGS/RESTORE_REGS exceed 32 instructions, each vector entry
// branches to an out-of-line handler.
//
// Kernel is compiled -neon,-fp-armv8; re-enable FP+SIMD at the asm level so
// the (unused-but-retained) SAVE_FP_REGS / RESTORE_FP_REGS macros below
// assemble cleanly.  FP save/restore itself has moved to do_switch_thread.
.arch armv8-a+fp+simd

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
    // Phase 1 (Linux fs .ko compat): set sp_el0 to the per-CPU
    // KabiTaskMock so kernel-mode code that reads `mrs xN, sp_el0`
    // (Linux's `current` macro on arm64) gets a Linux-shaped task
    // struct with stack_canary at +1912.  `kabi_task_mock_ptr`
    // lives at offset 32 in CpuLocalHead (see
    // `platform/arm64/cpu_local.rs`).  Zero = mock not installed
    // (very early boot, or AP that hasn't run install_for_current_cpu);
    // skip in that case.
    mrs     x0, tpidr_el1
    ldr     x0, [x0, #32]            // CpuLocalHead.kabi_task_mock_ptr
    cbz     x0, 1f
    msr     sp_el0, x0
1:
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

// FP/NEON register save/restore.
//
// The kernel is compiled -neon,-fp-armv8 (see kernel/arch/arm64/arm64.json),
// so EL1 code never writes v-regs.  User FP state therefore survives
// EL0→EL1→EL0 round-trips untouched — no save needed on exception entry.
// FP state is saved/restored only when we actually swap tasks, inside
// `do_switch_thread` (platform/arm64/usermode.S).  That matches Linux arm64's
// `-mgeneral-regs-only` + `fpsimd_save_state`/`fpsimd_load_state`.
//
// The macros below are retained for the (currently unused) paths that may
// want to snapshot HW FP onto a stack frame — see also `kevlar_save_fp_to`
// in usermode.S for the pointer-based variant used by fork().
//
// Frame layout (528 bytes, 16-byte aligned):
//   [sp+0]   .. [sp+511]  : v0-v31  (32 × 16 bytes)
//   [sp+512] .. [sp+519]  : FPCR
//   [sp+520] .. [sp+527]  : FPSR

.equ FP_FRAME_SZ, 528

.macro SAVE_FP_REGS
    sub     sp, sp, #FP_FRAME_SZ
    stp     q0,  q1,  [sp, #(0  * 32)]
    stp     q2,  q3,  [sp, #(1  * 32)]
    stp     q4,  q5,  [sp, #(2  * 32)]
    stp     q6,  q7,  [sp, #(3  * 32)]
    stp     q8,  q9,  [sp, #(4  * 32)]
    stp     q10, q11, [sp, #(5  * 32)]
    stp     q12, q13, [sp, #(6  * 32)]
    stp     q14, q15, [sp, #(7  * 32)]
    stp     q16, q17, [sp, #(8  * 32)]
    stp     q18, q19, [sp, #(9  * 32)]
    stp     q20, q21, [sp, #(10 * 32)]
    stp     q22, q23, [sp, #(11 * 32)]
    stp     q24, q25, [sp, #(12 * 32)]
    stp     q26, q27, [sp, #(13 * 32)]
    stp     q28, q29, [sp, #(14 * 32)]
    stp     q30, q31, [sp, #(15 * 32)]
    mrs     x0, fpcr
    mrs     x1, fpsr
    str     x0,        [sp, #512]
    str     x1,        [sp, #520]
.endm

.macro RESTORE_FP_REGS
    ldr     x0,        [sp, #512]
    ldr     x1,        [sp, #520]
    msr     fpcr, x0
    msr     fpsr, x1
    ldp     q0,  q1,  [sp, #(0  * 32)]
    ldp     q2,  q3,  [sp, #(1  * 32)]
    ldp     q4,  q5,  [sp, #(2  * 32)]
    ldp     q6,  q7,  [sp, #(3  * 32)]
    ldp     q8,  q9,  [sp, #(4  * 32)]
    ldp     q10, q11, [sp, #(5  * 32)]
    ldp     q12, q13, [sp, #(6  * 32)]
    ldp     q14, q15, [sp, #(7  * 32)]
    ldp     q16, q17, [sp, #(8  * 32)]
    ldp     q18, q19, [sp, #(9  * 32)]
    ldp     q20, q21, [sp, #(10 * 32)]
    ldp     q22, q23, [sp, #(11 * 32)]
    ldp     q24, q25, [sp, #(12 * 32)]
    ldp     q26, q27, [sp, #(13 * 32)]
    ldp     q28, q29, [sp, #(14 * 32)]
    ldp     q30, q31, [sp, #(15 * 32)]
    add     sp, sp, #FP_FRAME_SZ
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

// Kernel-mode exceptions: no FP save needed (kernel compiled with -neon).
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

// User-mode exceptions.  FP/NEON state is NOT saved here — the kernel is
// compiled -neon,-fp-armv8 and cannot clobber v-regs, so user FP state is
// preserved across the EL0→EL1→EL0 round-trip without our help.  FP save
// happens only on task swap, in do_switch_thread.
handle_lower_a64_sync:
    SAVE_REGS
    mov     x0, #1          // from_user = true
    mov     x1, sp           // PtRegs pointer
    bl      arm64_handle_exception
    // Deliver any pending signal before returning to user.
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
