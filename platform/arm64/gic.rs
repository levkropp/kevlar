// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! GICv2 interrupt controller driver for QEMU virt machine.
//! Distributor: 0x0800_0000, CPU interface: 0x0801_0000.
use super::KERNEL_BASE_ADDR;

const GICD_BASE_PHYS: usize = 0x0800_0000;
const GICC_BASE_PHYS: usize = 0x0801_0000;

fn gicd_base() -> usize {
    KERNEL_BASE_ADDR + GICD_BASE_PHYS
}

fn gicc_base() -> usize {
    KERNEL_BASE_ADDR + GICC_BASE_PHYS
}

unsafe fn mmio_read(addr: usize) -> u32 {
    core::ptr::read_volatile(addr as *const u32)
}

unsafe fn mmio_write(addr: usize, val: u32) {
    core::ptr::write_volatile(addr as *mut u32, val);
}

// Distributor registers.
const GICD_CTLR: usize = 0x000;
const GICD_ISENABLER: usize = 0x100;
const GICD_ICENABLER: usize = 0x180;
const GICD_ICPENDR: usize = 0x280;
const GICD_IPRIORITYR: usize = 0x400;
const GICD_ITARGETSR: usize = 0x800;
const GICD_ICFGR: usize = 0xC00;

// CPU interface registers.
const GICC_CTLR: usize = 0x000;
const GICC_PMR: usize = 0x004;
const GICC_IAR: usize = 0x00C;
const GICC_EOIR: usize = 0x010;

pub unsafe fn init() {
    let gicd = gicd_base();
    let gicc = gicc_base();

    // Disable distributor.
    mmio_write(gicd + GICD_CTLR, 0);

    // Set all SPIs to level-triggered, target CPU0, priority 0xa0.
    for i in (32..256).step_by(4) {
        mmio_write(gicd + GICD_IPRIORITYR + i, 0xa0a0a0a0);
    }
    for i in (32..256).step_by(4) {
        mmio_write(gicd + GICD_ITARGETSR + i, 0x01010101);
    }
    for i in (32..256).step_by(4) {
        mmio_write(gicd + GICD_ICFGR + (i / 16) * 4, 0);
    }

    // Enable distributor.
    mmio_write(gicd + GICD_CTLR, 1);

    // CPU interface: set priority mask to accept all, enable.
    mmio_write(gicc + GICC_PMR, 0xFF);
    mmio_write(gicc + GICC_CTLR, 1);
}

/// Per-AP GIC CPU interface initialization.
/// The distributor is already configured by the BSP; each AP only needs
/// to enable its own CPU interface.
pub unsafe fn init_ap() {
    let gicc = gicc_base();
    mmio_write(gicc + GICC_PMR, 0xFF);
    mmio_write(gicc + GICC_CTLR, 1);
}

/// Enable a specific IRQ (SPI number, e.g. 33 for UART).
pub fn enable_irq(irq: u8) {
    let gicd = gicd_base();
    let reg = (irq as usize) / 32;
    let bit = (irq as usize) % 32;
    unsafe {
        mmio_write(gicd + GICD_ISENABLER + reg * 4, 1 << bit);
    }
}

/// Disable a specific IRQ.
pub fn disable_irq(irq: u8) {
    let gicd = gicd_base();
    let reg = (irq as usize) / 32;
    let bit = (irq as usize) % 32;
    unsafe {
        mmio_write(gicd + GICD_ICENABLER + reg * 4, 1 << bit);
    }
}

/// Read the interrupt acknowledge register. Returns the IRQ number.
pub fn ack_interrupt() -> u32 {
    let gicc = gicc_base();
    unsafe { mmio_read(gicc + GICC_IAR) }
}

/// Signal end-of-interrupt.
pub fn end_interrupt(irq: u32) {
    let gicc = gicc_base();
    unsafe {
        mmio_write(gicc + GICC_EOIR, irq);
    }
}
