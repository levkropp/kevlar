// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! ARM64 syscall handling.
//! Linux ARM64 syscall ABI: x8 = syscall number, x0-x5 = args, x0 = return.
use crate::handler;

/// ARM64 register frame saved on exception entry.
/// Layout must match the SAVE_REGS / RESTORE_REGS macros in trap.S.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PtRegs {
    pub regs: [u64; 31], // x0-x30
    pub sp: u64,         // sp_el0 (user stack pointer)
    pub pc: u64,         // elr_el1 (return address)
    pub pstate: u64,     // spsr_el1
}

/// Called from trap.S for SVC (syscall) exceptions.
#[unsafe(no_mangle)]
pub extern "C" fn arm64_handle_syscall(frame: *mut PtRegs) -> isize {
    let f = unsafe { &*frame };
    let n = f.regs[8] as usize;    // syscall number
    let a1 = f.regs[0] as usize;   // arg1
    let a2 = f.regs[1] as usize;   // arg2
    let a3 = f.regs[2] as usize;   // arg3
    let a4 = f.regs[3] as usize;   // arg4
    let a5 = f.regs[4] as usize;   // arg5
    let a6 = f.regs[5] as usize;   // arg6
    handler().handle_syscall(a1, a2, a3, a4, a5, a6, n, frame)
}
