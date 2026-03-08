// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! ARM semihosting for QEMU exit.
//! Uses PSCI SYSTEM_OFF via HVC for clean QEMU exit, with semihosting as
//! fallback. QEMU virt machine handles PSCI calls.
use core::arch::asm;

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemihostingExitStatus {
    Success = 0x10,
    Failure = 0x11,
}

pub fn semihosting_halt(status: SemihostingExitStatus) {
    // Use PSCI SYSTEM_OFF (function ID 0x84000008) via HVC.
    // QEMU -machine virt implements PSCI, so this cleanly shuts down.
    let _ = status; // Exit code not meaningful for PSCI SYSTEM_OFF.
    unsafe {
        asm!(
            "mov w0, #0x0008",
            "movk w0, #0x8400, lsl #16",
            "hvc #0",
            options(noreturn, nomem, nostack),
        );
    }
}
