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

/// Called from trap.S for synchronous exceptions.
/// `from_user`: 0 = kernel, 1 = user.
#[unsafe(no_mangle)]
extern "C" fn arm64_handle_exception(_from_user: u64, frame: *mut PtRegs) {
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
            let ret = super::syscall::arm64_handle_syscall(frame);
            // Store return value in x0.
            unsafe { (*frame).regs[0] = ret as u64; }
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
                panic!(
                    "kernel page fault: pc={:#x}, far={:#x}, esr={:#x}",
                    pc, far, esr
                );
            }
        }
        _ => {
            let pc = unsafe { (*frame).pc };
            panic!(
                "unhandled synchronous exception: ec={:#x}, esr={:#x}, pc={:#x}, far={:#x}",
                ec, esr, pc, far
            );
        }
    }
}

/// Called from trap.S for IRQ exceptions.
#[unsafe(no_mangle)]
extern "C" fn arm64_handle_irq(_irq_type: u64, _frame: *mut PtRegs) {
    let irq = gic::ack_interrupt();
    let irq_id = irq & 0x3FF; // Mask off CPU ID bits.

    if irq_id >= 1020 {
        // Spurious interrupt — no action needed.
        return;
    }

    match irq_id as u8 {
        TIMER_IRQ => {
            timer::rearm();
            handler().handle_timer_irq();
        }
        UART_IRQ => {
            uart_irq_handler();
        }
        other => {
            // Mask the IRQ to prevent flooding from unhandled level-triggered
            // interrupts (e.g., virtio devices asserting before driver is ready).
            gic::disable_irq(other);
            handler().handle_irq(other);
        }
    }

    gic::end_interrupt(irq);
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
