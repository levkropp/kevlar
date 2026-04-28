// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! ARM64 exception and IRQ dispatch.
use crate::{address::UserVAddr, handler};
use core::arch::asm;

use super::{
    gic,
    paging::PageFaultReason,
    serial::{uart_irq_handler, UART_IRQ},
    syscall::PtRegs,
    timer::{self, TIMER_IRQ},
};

unsafe extern "C" {
    fn usercopy_start();
    fn usercopy_end();
}

// ESR_EL1 exception class values.
const EC_SVC_A64: u32 = 0b010101;     // SVC from AArch64
const EC_DATA_ABORT_LOWER: u32 = 0b100100;  // Data abort from lower EL
const EC_DATA_ABORT_CURR: u32 = 0b100101;   // Data abort from current EL
const EC_INST_ABORT_LOWER: u32 = 0b100000;  // Instruction abort from lower EL
const EC_INST_ABORT_CURR: u32 = 0b100001;   // Instruction abort from current EL
const EC_FP_ASIMD: u32 = 0b000111;    // Access to Advanced SIMD / FP while CPACR.FPEN traps

/// Called from trap.S for synchronous exceptions.
/// `from_user`: 0 = kernel, 1 = user.
#[unsafe(no_mangle)]
extern "C" fn arm64_handle_exception(from_user: u64, frame: *mut PtRegs) {
    let esr: u64;
    let far: u64;
    unsafe {
        asm!("mrs {}, esr_el1", out(reg) esr);
        asm!("mrs {}, far_el1", out(reg) far);
    }

    let ec = ((esr >> 26) & 0x3F) as u32;

    match ec {
        EC_SVC_A64 => {
            // Syscall from user space.
            // The dispatch code writes the return value directly to
            // frame.regs[0] AND handles signal delivery (which may
            // overwrite regs[0] with the signal number).  Do NOT
            // overwrite regs[0] here — it would clobber the signal
            // number set by try_delivering_signal().
            super::syscall::arm64_handle_syscall(frame);
        }
        EC_FP_ASIMD => {
            // EL0 tried to use FP/SIMD while CPACR.FPEN = 0b01 (trap
            // EL0).  Lazily restore this task's FpState; `eret` then
            // re-executes the faulting instruction.  See
            // platform/arm64/fp.rs for the state machine.
            super::fp::handle_fp_trap();
        }
        EC_DATA_ABORT_LOWER | EC_INST_ABORT_LOWER => {
            // User-space page fault.
            let iss = (esr & 0x1FFFFFF) as u32;
            let is_write = (iss >> 6) & 1 != 0;
            let is_inst = ec == EC_INST_ABORT_LOWER;

            let mut reason = PageFaultReason::CAUSED_BY_USER;
            if is_write {
                reason |= PageFaultReason::CAUSED_BY_WRITE;
            }
            if is_inst {
                reason |= PageFaultReason::CAUSED_BY_INST_FETCH;
            }
            // Check DFSC/IFSC for translation vs permission fault.
            let fsc = iss & 0x3F;
            if fsc >= 0x09 && fsc <= 0x0F {
                // Permission fault → page was present.
                reason |= PageFaultReason::PRESENT;
            }

            // Stash registers for crash diagnostics.  The generic crash
            // reporter uses x86-style names (rax/rbx/...); we map arm64
            // x0..x7 into those slots since that's what the dump printer
            // shows first.  x0..x7 cover the ARM AAPCS argument and
            // return registers, which is what crash analysis cares about.
            let f = unsafe { &*frame };
            let values: [u64; 19] = [
                f.regs[0],  // RAX slot <- x0
                f.regs[1],  // RBX slot <- x1
                f.regs[2],  // RCX slot <- x2
                f.regs[3],  // RDX slot <- x3
                f.regs[4],  // RSI slot <- x4
                f.regs[5],  // RDI slot <- x5
                f.regs[6],  // RBP slot <- x6
                f.sp,       // RSP slot <- SP_EL0 (user stack)
                f.regs[7],  // R8 slot  <- x7
                f.regs[8],  // R9 slot  <- x8 (syscall num)
                f.regs[9],  // R10
                f.regs[10], // R11
                f.regs[11], // R12
                f.regs[12], // R13
                f.regs[13], // R14
                f.regs[14], // R15
                f.pc,       // RIP
                f.pstate,   // RFLAGS slot <- SPSR
                far,        // FAULT_ADDR
            ];
            crate::crash_regs::stash(crate::arch::cpu_id() as usize, &values);

            let unaligned_vaddr = UserVAddr::new(far as usize);
            let pc = unsafe { (*frame).pc as usize };
            handler().handle_page_fault(unaligned_vaddr, pc, reason);
        }
        EC_DATA_ABORT_CURR | EC_INST_ABORT_CURR => {
            // Kernel page fault. Check if it's from usercopy.
            let pc = unsafe { (*frame).pc };
            let uc_start = usercopy_start as *const u8 as u64;
            let uc_end = usercopy_end as *const u8 as u64;
            let occurred_in_usercopy = pc >= uc_start && pc < uc_end;

            if occurred_in_usercopy {
                let unaligned_vaddr = UserVAddr::new(far as usize);
                let iss = (esr & 0x1FFFFFF) as u32;
                let is_write = (iss >> 6) & 1 != 0;
                let mut reason = PageFaultReason::empty();
                if is_write {
                    reason |= PageFaultReason::CAUSED_BY_WRITE;
                }
                handler().handle_page_fault(unaligned_vaddr, pc as usize, reason);
            } else {
                // Log the fault details via warn! BEFORE panicking — the
                // panic_handler only prints "[PANIC] CPU=N at file:line"
                // (PanicInfo::fmt can recursively crash on corrupt
                // panics), so the panic!() format args get lost.  This
                // ensures the pc/far/esr land on serial.
                log::warn!(
                    "KERNEL PAGE FAULT: cpu={} pc={:#x} far={:#x} esr={:#x}",
                    crate::arch::cpu_id(), pc, far, esr,
                );
                panic!(
                    "kernel page fault: pc={:#x}, far={:#x}, esr={:#x}",
                    pc, far, esr
                );
            }
        }
        _ => {
            let pc = unsafe { (*frame).pc };
            if from_user != 0 {
                // EL0 took an unclassified synchronous exception.  ec=0
                // is typically an undefined instruction (or some arm64
                // extension instruction HVF doesn't model).  Linux
                // delivers SIGILL/SIGSEGV in this situation; we hand
                // off to the kernel's `handle_user_fault` (which sends
                // a fatal signal to the current process) so the
                // misbehaving process dies cleanly without taking the
                // whole kernel down.
                //
                // Read the 4 instruction bytes at PC from the user
                // address space so we can decode what HVF refused.
                // This is best-effort: if PC isn't readable we just
                // report 0.  Knowing the instruction tells us whether
                // it's an arm64 v8.x extension (SVE/MTE/PAC/etc.)
                // that needs a CPACR_EL1 / HCR_EL2 enable bit, or a
                // real undefined opcode (genuine user-space bug).
                let mut insn_bytes = [0u8; 4];
                let uva_res = crate::address::UserVAddr::new_nonnull(pc as usize);
                let insn = if let Ok(uva) = uva_res {
                    if uva.read_bytes(&mut insn_bytes).is_ok() {
                        u32::from_le_bytes(insn_bytes)
                    } else {
                        0
                    }
                } else {
                    0
                };
                let pstate = unsafe { (*frame).pstate };
                let spsr_m = pstate & 0xf;
                let from_el = match spsr_m {
                    0b0000 => "EL0",
                    0b0100 => "EL1t",
                    0b0101 => "EL1h",
                    _ => "?",
                };
                log::warn!(
                    "EL0 unhandled exception: ec={:#x} esr={:#x} pc={:#x} far={:#x} \
                     insn={:#010x} pstate={:#x}({}) sp={:#x} \
                     — delivering signal to current process",
                    ec, esr, pc, far, insn, pstate, from_el, unsafe { (*frame).sp },
                );
                handler().handle_user_fault("arm64 EC=0 unknown", pc as usize);
                return;
            }
            panic!(
                "kernel unhandled synchronous exception: ec={:#x}, esr={:#x}, pc={:#x}, far={:#x}",
                ec, esr, pc, far
            );
        }
    }
}

/// Per-CPU EL0 PC sampler.
///
/// On every IRQ entry we bump `irq_count`.  On every IRQ that
/// interrupted EL0 (userspace), we also copy the saved PC/SP/PSTATE
/// into the sample slots and snapshot `irq_count` into
/// `last_el0_irq_count`.  Staleness of the EL0 sample is then
/// `irq_count - last_el0_irq_count` — high values mean the CPU has
/// been running EL1 (kernel/idle) for many IRQs since we last saw
/// it in userspace, so the PC sample is old.
///
/// This replaces the earlier `LAST_IRQ_FRAME[]` pointer scheme which
/// did not invalidate when the CPU went idle and would keep
/// reporting the same stale PC for seconds.
pub struct PerCpuIrqState {
    pub irq_count: core::sync::atomic::AtomicU64,
    pub last_el0_irq_count: core::sync::atomic::AtomicU64,
    pub el0_pc: core::sync::atomic::AtomicU64,
    pub el0_sp: core::sync::atomic::AtomicU64,
    pub el0_pstate: core::sync::atomic::AtomicU64,
    /// x30 = link register at IRQ time, i.e. the return address of
    /// the most recent BL.  When we sample inside memcpy/memset
    /// this points at the *caller* — invaluable for identifying
    /// which Xorg subsystem is looping.
    pub el0_lr: core::sync::atomic::AtomicU64,
    /// x0/x1/x2 at IRQ time.  For memcpy these are dst, src, len.
    pub el0_x0: core::sync::atomic::AtomicU64,
    pub el0_x1: core::sync::atomic::AtomicU64,
    pub el0_x2: core::sync::atomic::AtomicU64,
}

impl PerCpuIrqState {
    const fn new() -> Self {
        Self {
            irq_count: core::sync::atomic::AtomicU64::new(0),
            last_el0_irq_count: core::sync::atomic::AtomicU64::new(0),
            el0_pc: core::sync::atomic::AtomicU64::new(0),
            el0_sp: core::sync::atomic::AtomicU64::new(0),
            el0_pstate: core::sync::atomic::AtomicU64::new(0),
            el0_lr: core::sync::atomic::AtomicU64::new(0),
            el0_x0: core::sync::atomic::AtomicU64::new(0),
            el0_x1: core::sync::atomic::AtomicU64::new(0),
            el0_x2: core::sync::atomic::AtomicU64::new(0),
        }
    }
}

pub static PER_CPU_IRQ_STATE: [PerCpuIrqState; 8] = [
    PerCpuIrqState::new(), PerCpuIrqState::new(),
    PerCpuIrqState::new(), PerCpuIrqState::new(),
    PerCpuIrqState::new(), PerCpuIrqState::new(),
    PerCpuIrqState::new(), PerCpuIrqState::new(),
];

/// Read the most recent EL0 PC sample for the given CPU.
/// Returns (pc, sp, pstate, staleness_in_irqs) or None if no EL0
/// sample has ever been recorded.  `staleness_in_irqs == 0` means
/// the CPU is currently running EL0 code (or just trapped from it
/// in this very IRQ); higher values mean N IRQs have fired without
/// an EL0 source since the last sample.
pub fn last_user_state(cpu: usize) -> Option<(u64, u64, u64, u64)> {
    use core::sync::atomic::Ordering::Relaxed;
    if cpu >= PER_CPU_IRQ_STATE.len() { return None; }
    let s = &PER_CPU_IRQ_STATE[cpu];
    let last_el0 = s.last_el0_irq_count.load(Relaxed);
    if last_el0 == 0 { return None; }
    let now = s.irq_count.load(Relaxed);
    let pc = s.el0_pc.load(Relaxed);
    let sp = s.el0_sp.load(Relaxed);
    let pstate = s.el0_pstate.load(Relaxed);
    Some((pc, sp, pstate, now.saturating_sub(last_el0)))
}

/// Read the most recent EL0 register snapshot for the given CPU.
/// Returns (lr, x0, x1, x2) or None — companion to `last_user_state`.
pub fn last_user_regs(cpu: usize) -> Option<(u64, u64, u64, u64)> {
    use core::sync::atomic::Ordering::Relaxed;
    if cpu >= PER_CPU_IRQ_STATE.len() { return None; }
    let s = &PER_CPU_IRQ_STATE[cpu];
    if s.last_el0_irq_count.load(Relaxed) == 0 { return None; }
    Some((
        s.el0_lr.load(Relaxed),
        s.el0_x0.load(Relaxed),
        s.el0_x1.load(Relaxed),
        s.el0_x2.load(Relaxed),
    ))
}

/// Called from trap.S for IRQ exceptions.
#[unsafe(no_mangle)]
extern "C" fn arm64_handle_irq(_irq_type: u64, frame: *mut PtRegs) {
    {
        use core::sync::atomic::Ordering::Relaxed;
        let cpu = super::cpu_id() as usize;
        if cpu < PER_CPU_IRQ_STATE.len() {
            let s = &PER_CPU_IRQ_STATE[cpu];
            let n = s.irq_count.fetch_add(1, Relaxed) + 1;
            // Only update the EL0 sample if we interrupted EL0.
            // PSTATE.M[3:0] == 0b0000 (=0) means SPSR captured EL0t
            // (user-mode) at IRQ time.  Anything else (EL1h=5, etc.)
            // means we interrupted kernel code — preserve the prior
            // EL0 sample so the consumer can reason about staleness.
            let f = unsafe { &*frame };
            if (f.pstate & 0xf) == 0 {
                s.el0_pc.store(f.pc, Relaxed);
                s.el0_sp.store(f.sp, Relaxed);
                s.el0_pstate.store(f.pstate, Relaxed);
                s.el0_lr.store(f.regs[30], Relaxed);
                s.el0_x0.store(f.regs[0], Relaxed);
                s.el0_x1.store(f.regs[1], Relaxed);
                s.el0_x2.store(f.regs[2], Relaxed);
                s.last_el0_irq_count.store(n, Relaxed);
            }
        }
    }
    let irq = gic::ack_interrupt();
    let irq_id = irq & 0x3FF; // Mask off CPU ID bits.

    if irq_id >= 1020 {
        // Spurious interrupt — no action needed.
        return;
    }

    match irq_id as u8 {
        TIMER_IRQ => {
            // CRITICAL: rearm the timer AND EOI the GIC BEFORE running
            // the timer handler, because the handler may call
            // `process::switch()` which transfers the CPU to a different
            // process.  If we EOI'd after the handler instead, and the
            // process whose IRQ stack we're parked on later gets killed
            // before being rescheduled, the original IRQ stack is leaked
            // along with the un-EOI'd GIC state — the timer IRQ stays
            // marked Active on this CPU and no further timer IRQs ever
            // deliver.  Symptom: per-CPU tick counter (`PER_CPU_TICKS`)
            // freezes mid-run and that CPU stops scheduling forever.
            // See blog 231 for the multi-day investigation.
            timer::rearm();
            gic::end_interrupt(irq);
            handler().handle_timer_irq();
            return;
        }
        gic::SGI_RESCHEDULE => {
            // Cross-CPU reschedule wake.  No work to do — the WFI exit
            // on entry to the IRQ vector already brought us out of
            // idle, and the normal preemption tick (or the next return
            // from this IRQ) will pick up whatever was just enqueued
            // on our run-queue by the sender.
        }
        gic::SGI_MEMBARRIER => {
            // membarrier(MEMBARRIER_CMD_GLOBAL) IPI — issue a full
            // system memory barrier so prior user-space stores on the
            // originating CPU become visible to user-space code that
            // runs on this CPU after the IRQ returns.  `dsb sy` is
            // the heaviest barrier arm64 has — it orders all loads
            // and stores from all observers before any after.
            unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
        }
        UART_IRQ => {
            uart_irq_handler();
        }
        other => {
            // Run the driver's handler.  It is responsible for
            // quiescing the device (e.g. virtio-mmio drivers must
            // read InterruptStatus and write InterruptACK so the
            // device de-asserts its level-triggered line) before we
            // EOI the GIC, otherwise the same IRQ would re-fire
            // immediately.  We do NOT mask the IRQ at the GIC: doing
            // so silently kills the line for the rest of the run, so
            // the *first* virtio-input interrupt would be the only
            // one ever delivered, and steady-state event flow stops.
            // (Pre-handler masking was a band-aid against unattached
            // devices flooding IRQs at boot — but the only IRQs that
            // get here at all already have an `attach_irq` handler
            // registered, so there's nothing to flood.)
            handler().handle_irq(other);
        }
    }

    gic::end_interrupt(irq);
}

/// Called from trap.S after returning from a lower-EL exception or IRQ,
/// before RESTORE_REGS + eret.  Delivers any pending signal to the current
/// process by redirecting ELR_EL1 to the signal trampoline.
///
/// Mirrors x86_64's `x64_check_signal_on_irq_return`.
#[unsafe(no_mangle)]
extern "C" fn arm64_check_signal_on_return(frame: *mut super::syscall::PtRegs) {
    if handler().current_process_signal_pending() == 0 {
        return;
    }
    handler().handle_interrupt_return(frame as *mut _);
}

/// Called for unhandled exceptions (FIQ, SError, AArch32, SP_EL0).
#[unsafe(no_mangle)]
extern "C" fn arm64_unhandled_exception(esr: u64, elr: u64, far: u64) {
    panic!(
        "unhandled exception: esr={:#x}, elr={:#x}, far={:#x}",
        esr, elr, far
    );
}

/// Saved interrupt status (DAIF flags).
pub struct SavedInterruptStatus {
    daif: u64,
}

impl SavedInterruptStatus {
    pub fn save() -> SavedInterruptStatus {
        let daif: u64;
        unsafe { asm!("mrs {}, daif", out(reg) daif) };
        SavedInterruptStatus { daif }
    }
}

impl Drop for SavedInterruptStatus {
    fn drop(&mut self) {
        // Restore the IRQ mask bit.
        if self.daif & (1 << 7) == 0 {
            // IRQs were enabled before — re-enable.
            unsafe { asm!("msr daifclr, #2") };
        }
    }
}
