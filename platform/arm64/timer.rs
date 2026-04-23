// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! ARM generic timer driver.
//!
//! Uses CNTV (virtual timer) at EL1.  Apple's Hypervisor.framework traps
//! every CNTP_* (physical timer) access as UNDEFINED for EL1 guests — only
//! the hypervisor gets the physical timer — so we must drive the virtual
//! timer to boot under HVF.  Linux-on-arm64 does the same for the same
//! reason, and TCG / KVM-on-Linux both accept CNTV happily, so there's no
//! reason to keep a CNTP path at all.
//!
//! Timer IRQ = PPI 11 (virtual timer) → GIC IRQ 27.
use core::arch::asm;

pub const TIMER_IRQ: u8 = 27; // PPI 11 = GIC IRQ 27 (virtual timer)

/// Read the counter frequency (CNTFRQ_EL0 — shared by physical and virtual).
fn cntfrq() -> u64 {
    let val: u64;
    unsafe { asm!("mrs {}, cntfrq_el0", out(reg) val) };
    val
}

/// Public wrapper for `read_clock_frequency` in profile.rs.
pub fn cntfrq_public() -> u64 { cntfrq() }

/// Cached tval to avoid re-reading CNTFRQ on every rearm.
static mut TVAL: u64 = 0;

/// Initialize the timer to fire at TICK_HZ.
pub unsafe fn init() {
    let freq = cntfrq();
    let tval = freq / super::TICK_HZ as u64;

    unsafe { TVAL = tval; }

    // Set timer value and enable.
    asm!("msr cntv_tval_el0, {}", in(reg) tval);
    // CNTV_CTL_EL0: ENABLE=1, IMASK=0
    asm!("msr cntv_ctl_el0, {}", in(reg) 1u64);

    // Enable the virtual timer PPI in the GIC.
    super::gic::enable_irq(TIMER_IRQ);
}

/// Per-AP timer initialization.
/// TVAL is already set by the BSP; each AP just starts its own countdown
/// and enables the PPI in the GIC CPU interface.
/// Must be called after `gic::init_ap()` and `process::init_ap()`.
pub unsafe fn init_ap() {
    let tval = unsafe { TVAL };
    // Set countdown and enable.
    asm!("msr cntv_tval_el0, {}", in(reg) tval);
    asm!("msr cntv_ctl_el0, {}", in(reg) 1u64);
    // Enable the virtual-timer PPI in this CPU's GIC banked register.
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
        asm!("msr cntv_ctl_el0, {}", in(reg) 3u64);
        // Set new countdown (this also updates CVAL, clearing ISTATUS
        // once the new CVAL is in the future).
        asm!("msr cntv_tval_el0, {}", in(reg) tval);
        // Unmask.  ISTATUS should now be 0 since CVAL is in the future.
        // CTL = ENABLE = 0b01 = 1
        asm!("msr cntv_ctl_el0, {}", in(reg) 1u64);
    }
}

/// Read the current counter value (monotonic, virtual timebase).
pub fn counter() -> u64 {
    let val: u64;
    unsafe { asm!("mrs {}, cntvct_el0", out(reg) val) };
    val
}

/// Nanoseconds since boot derived from CNTPCT_EL0.  Used by the monotonic
/// clock so /proc/uptime and clock_gettime(MONOTONIC) have sub-tick
/// resolution — otherwise a process reading /proc/uptime within the first
/// timer tick sees 0.00 and contract tests requiring uptime > 0 fail.
pub fn nanoseconds_since_boot() -> u64 {
    let freq = cntfrq();
    if freq == 0 {
        return 0;
    }
    let cnt = counter();
    // Compute cnt * 1e9 / freq without overflow.  Typical freq is 24 MHz,
    // so cnt * 1e9 overflows u64 at ~18.5 s — use u128 for the multiply.
    ((cnt as u128) * 1_000_000_000u128 / (freq as u128)) as u64
}
