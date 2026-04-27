// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use core::arch::asm;

use super::semihosting::{semihosting_halt, SemihostingExitStatus};

pub fn idle() {
    unsafe {
        // Defensive re-arm: if this CPU's virtual timer is no longer
        // enabled+unmasked (e.g. masked off by a fault return path,
        // or disabled by some user-code-triggered control flow we
        // haven't traced yet), revive it so the next tick can fire.
        // CRITICAL: do NOT rearm unconditionally — that would reset
        // the countdown on every idle iteration and the timer would
        // never reach zero.  Only rearm when the CTL register no
        // longer reads back ENABLE (bit 0) or IMASK is set (bit 1).
        // See blog 230 listener-starvation notes for the symptom.
        let ctl: u64;
        core::arch::asm!("mrs {}, cntv_ctl_el0", out(reg) ctl);
        // Healthy: CTL = 0b001 (ENABLE=1, IMASK=0, ISTATUS=don't-care).
        // Unhealthy: ENABLE=0 (timer disabled) or IMASK=1 (output masked).
        if (ctl & 0b011) != 0b001 {
            super::timer::rearm();
        }

        // Enable interrupts, wait for interrupt, disable interrupts.
        asm!("msr daifclr, #2"); // unmask IRQ
        asm!("wfi");
        asm!("msr daifset, #2"); // mask IRQ
    }
}

#[cfg_attr(test, allow(unused))]
pub fn halt() -> ! {
    semihosting_halt(SemihostingExitStatus::Success);

    loop {
        unsafe {
            asm!("msr daifset, #0xf");
            asm!("wfi");
        }
    }
}
