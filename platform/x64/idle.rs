// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use core::arch::asm;

use super::semihosting::{semihosting_halt, SemihostingExitStatus};

pub fn idle() {
    crate::flight_recorder::record(crate::flight_recorder::kind::IDLE_ENTER, 0, 0, 0);
    unsafe {
        asm!("sti; hlt; cli");
    }
    crate::flight_recorder::record(crate::flight_recorder::kind::IDLE_EXIT, 0, 0, 0);
}

#[cfg_attr(test, allow(unused))]
pub fn halt() -> ! {
    semihosting_halt(SemihostingExitStatus::Success);

    loop {
        unsafe {
            asm!("cli; hlt");
        }
    }
}
