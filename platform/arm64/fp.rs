// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Lazy FP/NEON save+restore on arm64 (EC=0x07 trap handler).
//!
//! Policy — mirror of Linux `arch/arm64/kernel/fpsimd.c`:
//!   - Context switch does NOT save or load v-regs.  Outgoing task's FP
//!     state stays live in HW; CPACR_EL1.FPEN is flipped to trap EL0 FP.
//!   - First time the next task uses FP/SIMD at EL0, EC=0x07 fires.
//!     `handle_fp_trap` saves the previous owner's v-regs into its own
//!     `FpState` (if there was one), loads this task's `FpState` into
//!     v-regs, records this task as the new FP owner, and re-enables
//!     EL0 FP.  `eret` re-executes the faulting instruction.
//!
//! Correctness premise: the kernel is built `-fp-armv8 -neon`, so EL1
//! code NEVER reads or writes v-regs / FPCR / FPSR.  Between trap entry
//! and trap exit, v-regs are byte-preserved by hardware.  The only
//! things that change v-regs are (a) EL0 code and (b) explicit calls
//! to `kevlar_save_fp_to` / `kevlar_restore_fp_from` in this module.
//!
//! Single-CPU simplification: `fp_owner` is per-CPU.  On SMP with task
//! migration, a task's FpState may be stale w.r.t. a remote CPU's
//! v-regs.  Linux tracks `task.fpsimd_cpu` to detect this.  For now
//! we run single-CPU; SMP migration tracking is a follow-up.

use super::cpu_local::cpu_local_head;
use super::task::{kevlar_restore_fp_from, kevlar_save_fp_to, FpState};

/// CPACR_EL1.FPEN field values (bits [21:20]).
const CPACR_FPEN_TRAP_ALL: u64 = 0 << 20;      // Traps both EL0 and EL1.
const CPACR_FPEN_TRAP_EL0: u64 = 1 << 20;      // EL1 allowed, EL0 traps.
#[allow(dead_code)]
const CPACR_FPEN_ALLOW_ALL: u64 = 3 << 20;     // EL0 and EL1 allowed.

/// Set CPACR_EL1.FPEN = 0b11 — allow EL0 FP/SIMD access.
/// Called by `handle_fp_trap` after loading the current task's state so
/// the faulting instruction can be re-executed successfully.
#[inline(always)]
#[allow(unsafe_code)]
pub fn cpacr_allow_el0_fp() {
    unsafe {
        let cpacr: u64 = CPACR_FPEN_ALLOW_ALL;
        core::arch::asm!(
            "msr cpacr_el1, {x}",
            "isb",
            x = in(reg) cpacr,
            options(nostack),
        );
    }
}

/// Set CPACR_EL1.FPEN = 0b01 — trap EL0 FP/SIMD, allow EL1.
/// Called on context switch out so the incoming task's first FP use
/// traps into `handle_fp_trap`.
#[inline(always)]
#[allow(unsafe_code)]
pub fn cpacr_trap_el0_fp() {
    unsafe {
        let cpacr: u64 = CPACR_FPEN_TRAP_EL0;
        core::arch::asm!(
            "msr cpacr_el1, {x}",
            "isb",
            x = in(reg) cpacr,
            options(nostack),
        );
    }
}

/// Handle an EL0 FP/SIMD access trap (EC=0x07).
///
/// Called from `arm64_handle_exception` when userspace executes an FP
/// or NEON instruction while CPACR.FPEN is 0b01.
///
/// Invariants on entry:
///   - EL1 has not touched v-regs / FPCR / FPSR since the trap fired;
///     the hardware state STILL belongs to `cpu_local_head().fp_owner`
///     (the task that last had FP loaded on this CPU), if any.
///   - `current_process()` is the task that just trapped.
///
/// Postconditions:
///   - V-regs and FPCR/FPSR hold `current.arch.fp_state`.
///   - `cpu_local_head().fp_owner` points at `current.arch.fp_state`.
///   - Previous owner's `fp_state` has been saved from v-regs (if there
///     was a previous owner distinct from current).
///   - CPACR.FPEN = 0b11 (EL0 FP allowed for the `eret`).
/// Count of EC=0x07 FP traps handled.  Used as a sanity check that
/// the lazy-save scheme is actually engaging — if zero, Xorg's
/// NEON/SIMD instructions are running without ever loading the
/// task's saved FpState (the cause of memcpy infinite loops in
/// task #42).
pub static FP_TRAP_COUNT: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

#[allow(unsafe_code)]
pub fn handle_fp_trap() {
    FP_TRAP_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    // Disable preemption: we're about to mutate per-CPU and per-task FP
    // state non-atomically.  A timer-driven reschedule in the middle
    // would leave v-regs and `fp_owner` out of sync.
    crate::arch::preempt_disable();

    let current_u64 = crate::handler().current_task_fp_state_ptr();
    let current: *mut FpState = current_u64 as *mut FpState;
    if current.is_null() {
        // Process subsystem not ready — can't save/load.  Just allow FP
        // and return; the only caller in this state would be early-boot
        // test code before PID 1 exists.
        cpacr_allow_el0_fp();
        crate::arch::preempt_enable();
        return;
    }
    let head = cpu_local_head();
    let prev_owner: *mut FpState = head.fp_owner as *mut FpState;

    // Save the previous owner's v-regs, unless there was no owner or the
    // owner is already the current task (same task, same CPU, still
    // loaded — a redundant trap which should be rare).
    if !prev_owner.is_null() && prev_owner != current {
        unsafe { kevlar_save_fp_to(prev_owner); }
    }

    // Load the current task's saved FP state into HW.
    unsafe { kevlar_restore_fp_from(current); }

    // Record ownership and allow EL0 FP until the next context switch
    // flips CPACR back.
    head.fp_owner = current_u64;
    cpacr_allow_el0_fp();

    // Mark current as loaded.  The arm64 equivalent of clearing
    // `TIF_FOREIGN_FPSTATE` in Linux.  Used by the switch-in fast
    // path that avoids re-trapping if the same task lands back on
    // this CPU before another task touches FP.
    crate::handler().mark_current_task_fp_loaded();

    crate::arch::preempt_enable();

    // Suppress dead-code warnings until step 4 flips CPACR to trap mode.
    let _ = CPACR_FPEN_TRAP_ALL;
    let _ = CPACR_FPEN_TRAP_EL0;
}
