// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::address::PAddr;
use crate::spinlock::SpinLock;
use core::ptr::{read_volatile, write_volatile};
use x86::msr::{self, rdmsr, wrmsr};

/// The base index of interrupt vectors.
const APIC_BASE_EN: u64 = 1 << 11;
const SIVR_SOFT_EN: u32 = 1 << 8;

static APIC: SpinLock<LocalApic> = SpinLock::new(LocalApic::new(PAddr::new(0xfee0_0000)));

#[derive(Debug, Copy, Clone)]
#[repr(u32)]
enum LocalApicReg {
    Eoi = 0xb0,
    SpuriousInterrupt = 0xf0,
}

struct LocalApic {
    base: PAddr,
}

impl LocalApic {
    pub const fn new(base: PAddr) -> LocalApic {
        LocalApic { base }
    }

    pub unsafe fn write_eoi(&self) {
        // The EOI register accepts only 0. CPU raises #GP otherwise.
        self.mmio_write(LocalApicReg::Eoi, 0);
    }

    pub unsafe fn write_spurious_interrupt(&self, value: u32) {
        self.mmio_write(LocalApicReg::SpuriousInterrupt, value);
    }

    #[inline(always)]
    unsafe fn _mmio_read(&self, reg: LocalApicReg) -> u32 {
        read_volatile(self.base.add(reg as usize).as_ptr())
    }

    #[inline(always)]
    unsafe fn mmio_write(&self, reg: LocalApicReg, value: u32) {
        write_volatile(self.base.add(reg as usize).as_mut_ptr(), value)
    }
}

/// Acknowledge the current interrupt.
///
/// This is called from the interrupt handler on every IRQ.  We write the
/// EOI register directly instead of going through the SpinLock — this
/// kernel is single-CPU and the interrupt handler runs with interrupts
/// disabled, so the lock acquire/release was pure overhead (cli/sti +
/// deadlock check + backtrace capture in debug builds).
#[inline(always)]
pub fn ack_interrupt() {
    unsafe {
        // Safety: single-CPU, interrupts disabled in this context.
        // The APIC base (0xfee00000) is a hardware-fixed address that
        // never changes.  EOI register is at offset 0xb0.
        let eoi_addr = PAddr::new(0xfee0_00b0).as_vaddr();
        core::ptr::write_volatile(eoi_addr.as_mut_ptr::<u32>(), 0);
    }
}

pub unsafe fn init() {
    // Activate Local APIC.
    let apic_base = rdmsr(msr::APIC_BASE);
    wrmsr(msr::APIC_BASE, apic_base | APIC_BASE_EN);

    let apic = APIC.lock();
    apic.write_spurious_interrupt(SIVR_SOFT_EN);
}
