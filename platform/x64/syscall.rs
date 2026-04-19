// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::handler;

use super::gdt::{KERNEL_CS, USER_CS32};
use x86::msr::{self, rdmsr, wrmsr};

// Mask RFLAGS bits on SYSCALL entry (IA32_FMASK).  The CPU clears these
// bits in RFLAGS when SYSCALL executes:
//   IF  (0x0200) — disable interrupts before SWAPGS
//   TF  (0x0100) — prevent single-step #DB flood in kernel mode
//   DF  (0x0400) — direction flag (string ops forward)
// Note: Linux also masks NT and AC, but we keep it minimal for now.
const SYSCALL_RFLAGS_MASK: u64 = 0x0700;

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct PtRegs {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rax: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub orig_rax: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

#[unsafe(no_mangle)]
extern "C" fn x64_handle_syscall(
    a1: usize,
    a2: usize,
    a3: usize,
    a4: usize,
    a5: usize,
    a6: usize,
    n: usize,
    frame: *mut PtRegs,
) -> isize {
    let cpu = super::cpu_id() as usize;
    if cpu < SYSCALL_COUNT.len() {
        SYSCALL_COUNT[cpu].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        LAST_SYSCALL_NR[cpu].store(n as u32, core::sync::atomic::Ordering::Relaxed);
    }
    handler().handle_syscall(a1, a2, a3, a4, a5, a6, n, frame)
}

/// Per-CPU syscall counter, bumped on every syscall entry. Available via
/// `kevlar_platform::arch::syscall_counter_read(cpu)` — useful when
/// diagnosing whether a CPU has stopped making syscalls (e.g. IF=0 lockup).
pub static SYSCALL_COUNT: [core::sync::atomic::AtomicUsize; 8] = [
    core::sync::atomic::AtomicUsize::new(0), core::sync::atomic::AtomicUsize::new(0),
    core::sync::atomic::AtomicUsize::new(0), core::sync::atomic::AtomicUsize::new(0),
    core::sync::atomic::AtomicUsize::new(0), core::sync::atomic::AtomicUsize::new(0),
    core::sync::atomic::AtomicUsize::new(0), core::sync::atomic::AtomicUsize::new(0),
];

/// Per-CPU last-syscall-number, updated on every syscall entry. Combined
/// with `SYSCALL_COUNT`, exposes "CPU N is stuck inside syscall nr=X" to
/// diagnostic dumps without requiring a live debugger.
pub static LAST_SYSCALL_NR: [core::sync::atomic::AtomicU32; 8] = [
    core::sync::atomic::AtomicU32::new(0), core::sync::atomic::AtomicU32::new(0),
    core::sync::atomic::AtomicU32::new(0), core::sync::atomic::AtomicU32::new(0),
    core::sync::atomic::AtomicU32::new(0), core::sync::atomic::AtomicU32::new(0),
    core::sync::atomic::AtomicU32::new(0), core::sync::atomic::AtomicU32::new(0),
];

unsafe extern "C" {
    fn syscall_entry();
}

pub unsafe fn init() {
    wrmsr(
        msr::IA32_STAR,
        ((USER_CS32 as u64) << 48) | ((KERNEL_CS as u64) << 32),
    );
    wrmsr(msr::IA32_LSTAR, syscall_entry as *const u8 as u64);
    wrmsr(msr::IA32_FMASK, SYSCALL_RFLAGS_MASK);

    // RIP for compatibility mode. We don't support it for now.
    wrmsr(msr::IA32_CSTAR, 0);

    // Enable SYSCALL/SYSRET.
    wrmsr(msr::IA32_EFER, rdmsr(msr::IA32_EFER) | 1);
}
