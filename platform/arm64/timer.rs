// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! ARM generic timer driver.
//! Uses CNTP (physical timer) at EL1.
//! Timer IRQ = PPI 14 -> GIC IRQ 30.
use core::arch::asm;

pub const TIMER_IRQ: u8 = 30; // PPI 14 = GIC IRQ 30

/// Read the counter frequency (CNTFRQ_EL0).
fn cntfrq() -> u64 {
    let val: u64;
    unsafe { asm!("mrs {}, cntfrq_el0", out(reg) val) };
    val
}

/// Cached tval to avoid re-reading CNTFRQ on every rearm.
static mut TVAL: u64 = 0;

/// Initialize the timer to fire at TICK_HZ.
pub unsafe fn init() {
    let freq = cntfrq();
    let tval = freq / super::TICK_HZ as u64;

    unsafe { TVAL = tval; }

    // Set timer value and enable.
    asm!("msr cntp_tval_el0, {}", in(reg) tval);
    // CNTP_CTL_EL0: ENABLE=1, IMASK=0
    asm!("msr cntp_ctl_el0, {}", in(reg) 1u64);

    // Enable the timer IRQ in the GIC.
    super::gic::enable_irq(TIMER_IRQ);
}

/// Per-AP timer initialization.
/// TVAL is already set by the BSP; each AP just starts its own countdown
/// and enables the PPI in the GIC CPU interface.
/// Must be called after `gic::init_ap()` and `process::init_ap()`.
pub unsafe fn init_ap() {
    let tval = unsafe { TVAL };
    // Set countdown and enable.
    asm!("msr cntp_tval_el0, {}", in(reg) tval);
    asm!("msr cntp_ctl_el0, {}", in(reg) 1u64);
    // Enable the timer PPI in this CPU's GIC banked register.
    super::gic::enable_irq(TIMER_IRQ);
}

/// Rearm the timer for the next tick.
///
/// The timer is level-triggered: ISTATUS stays asserted until the compare
/// value moves past the current counter.  We mask the timer output (IMASK)
/// while we reprogram, then unmask after setting the new countdown.
pub fn rearm() {
    let tval = unsafe { TVAL };
    unsafe {
        // Mask timer output so GIC sees deasserted.
        // CTL = ENABLE | IMASK = 0b11 = 3
        asm!("msr cntp_ctl_el0, {}", in(reg) 3u64);
        // Set new countdown (this also updates CVAL, clearing ISTATUS
        // once the new CVAL is in the future).
        asm!("msr cntp_tval_el0, {}", in(reg) tval);
        // Unmask.  ISTATUS should now be 0 since CVAL is in the future.
        // CTL = ENABLE = 0b01 = 1
        asm!("msr cntp_ctl_el0, {}", in(reg) 1u64);
    }
}

/// Read the current counter value (monotonic).
pub fn counter() -> u64 {
    let val: u64;
    unsafe { asm!("mrs {}, cntpct_el0", out(reg) val) };
    val
}
