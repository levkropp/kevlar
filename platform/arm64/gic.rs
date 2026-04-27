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
    let v: u32;
    // Plain `ldr w, [x]` — no post-/pre-index.  HVF's EC_DATAABORT path
    // asserts ESR.ISV in qemu target/arm/hvf/hvf.c; post-indexed loads
    // clear ISV on Apple Silicon's stage-2 trap, crashing the hypervisor.
    // The inline-asm form fixes the encoding so HVF always sees a valid
    // syndrome.
    core::arch::asm!("ldr {v:w}, [{a}]", v = out(reg) v, a = in(reg) addr,
                     options(nostack, preserves_flags, readonly));
    v
}

unsafe fn mmio_write(addr: usize, val: u32) {
    // Plain `str w, [x]` — see `mmio_read` for rationale.
    core::arch::asm!("str {v:w}, [{a}]", v = in(reg) val, a = in(reg) addr,
                     options(nostack, preserves_flags));
}

// Distributor registers.
const GICD_CTLR: usize = 0x000;
const GICD_ISENABLER: usize = 0x100;
const GICD_ICENABLER: usize = 0x180;
const GICD_ICPENDR: usize = 0x280;
const GICD_IPRIORITYR: usize = 0x400;
const GICD_ITARGETSR: usize = 0x800;
const GICD_ICFGR: usize = 0xC00;
const GICD_SGIR: usize = 0xF00;

/// SGI we use for cross-CPU "wake up and reschedule".  The receive
/// handler is empty — the WFI exit on the target CPU is the point.
/// Any value 0..=15 works; pick 1 because 0 is sometimes special.
pub const SGI_RESCHEDULE: u8 = 1;

/// SGI we use for membarrier(2) — the target CPU executes a `dsb sy`
/// in the IRQ handler so that any user-space stores it issued before
/// the membarrier syscall on the originating CPU are observable to it
/// after the syscall returns.  The matching local barrier on the
/// originating CPU is issued in the syscall handler itself.
pub const SGI_MEMBARRIER: u8 = 2;

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

    // SGIs (0..15) and PPIs (16..31) live in IPRIORITYR0..IPRIORITYR7
    // (the first 32 bytes — one byte per IRQ).  Banked per-CPU.  Set
    // them all to the same priority as SPIs so the CPU interface
    // doesn't reject deliveries based on PMR.
    for i in (0..32).step_by(4) {
        mmio_write(gicd + GICD_IPRIORITYR + i, 0xa0a0a0a0);
    }
    // Enable our reschedule SGI so the BSP receives it from APs.
    // ISENABLER0 covers SGIs+PPIs; banked per-CPU.  enable_irq writes
    // to the CALLING CPU's view, so this enables it for the BSP.
    enable_irq(SGI_RESCHEDULE);
    enable_irq(SGI_MEMBARRIER);

    // Enable distributor.
    mmio_write(gicd + GICD_CTLR, 1);

    // CPU interface: set priority mask to accept all, enable.
    mmio_write(gicc + GICC_PMR, 0xFF);
    mmio_write(gicc + GICC_CTLR, 1);
}

/// Per-AP GIC CPU interface initialization.
/// The distributor is already configured by the BSP; each AP only needs
/// to enable its own CPU interface AND its banked PPI/SGI lines.
pub unsafe fn init_ap() {
    let gicc = gicc_base();
    mmio_write(gicc + GICC_PMR, 0xFF);
    mmio_write(gicc + GICC_CTLR, 1);
    // Enable the reschedule SGI on this AP — ISENABLER0 is banked.
    enable_irq(SGI_RESCHEDULE);
    enable_irq(SGI_MEMBARRIER);
}

/// Send the reschedule SGI to one specific target CPU.
///
/// `cpu_id` is the CPU's logical id (0..=7).  GICv2 SGI delivery uses
/// a CPUTargetList bitmap, so the conversion is a 1-bit shift.  The
/// receiving CPU takes the SGI as a regular IRQ in its 0..15 range
/// — `arm64_handle_irq` discards it (the value of an empty handler is
/// the wake-from-WFI side effect, not the work it does).
pub fn send_reschedule_ipi(cpu_id: u32) {
    debug_assert!(cpu_id < 8);
    let gicd = gicd_base();
    // GICD_SGIR layout (GICv2 §4.3.15):
    //   bits [25:24] TargetListFilter — 0b00 = use CPUTargetList bitmap
    //   bits [23:16] CPUTargetList   — 1-bit per target CPU (0..=7)
    //   bits [15] NSATT (0 = group 0, irrelevant for non-secure)
    //   bits [3:0]  SGIINTID         — 0..=15
    let cpu_bit = 1u32 << cpu_id;
    let val = (cpu_bit << 16) | (SGI_RESCHEDULE as u32);
    unsafe { mmio_write(gicd + GICD_SGIR, val); }
}

/// Broadcast a membarrier SGI to every other CPU.  Receivers execute
/// `dsb sy` in the IRQ path so any prior user-space stores on the
/// originating CPU become visible to them by the time the syscall
/// returns to user space.  Implements MEMBARRIER_CMD_GLOBAL semantics.
///
/// TargetListFilter=0b01 means "all CPUs except the requesting CPU"
/// (GICv2 §4.3.15) — exactly the membarrier broadcast set.
pub fn broadcast_membarrier_ipi() {
    let gicd = gicd_base();
    let val = (0b01u32 << 24) | (SGI_MEMBARRIER as u32);
    unsafe { mmio_write(gicd + GICD_SGIR, val); }
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
