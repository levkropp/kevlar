// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use core::arch::asm;

use super::semihosting::{semihosting_halt, SemihostingExitStatus};

pub fn idle() {
    unsafe {
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
