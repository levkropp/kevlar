// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Per-CPU register stash for crash diagnostics.
//!
//! The interrupt handler stores the faulting register state here before
//! dispatching to the page fault handler. If the fault is fatal (SIGSEGV),
//! the kernel reads these registers to include in the crash report.
//!
//! Only the most recent fault per CPU is stored. Since the page fault
//! handler runs with interrupts disabled, there is no race.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const MAX_CPUS: usize = 8;
/// Number of u64 values stored per CPU: 16 GP regs + rip + rsp + rflags = 19.
const REGS_PER_CPU: usize = 19;

static VALID: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];
static DATA: [[AtomicU64; REGS_PER_CPU]; MAX_CPUS] =
    [const { [const { AtomicU64::new(0) }; REGS_PER_CPU] }; MAX_CPUS];

/// Register indices — matches the order in `CrashRegs`.
const RAX: usize = 0;
const RBX: usize = 1;
const RCX: usize = 2;
const RDX: usize = 3;
const RSI: usize = 4;
const RDI: usize = 5;
const RBP: usize = 6;
const RSP: usize = 7;
const R8: usize = 8;
const R9: usize = 9;
const R10: usize = 10;
const R11: usize = 11;
const R12: usize = 12;
const R13: usize = 13;
const R14: usize = 14;
const R15: usize = 15;
const RIP: usize = 16;
const RFLAGS: usize = 17;
const FAULT_ADDR: usize = 18;

/// Stashed register state, returned by `take()`.
#[derive(Clone, Copy, Debug)]
pub struct CrashRegs {
    pub rax: u64, pub rbx: u64, pub rcx: u64, pub rdx: u64,
    pub rsi: u64, pub rdi: u64, pub rbp: u64, pub rsp: u64,
    pub r8: u64, pub r9: u64, pub r10: u64, pub r11: u64,
    pub r12: u64, pub r13: u64, pub r14: u64, pub r15: u64,
    pub rip: u64, pub rflags: u64, pub fault_addr: u64,
}

/// Store register values for the given CPU. Called from the interrupt
/// handler before dispatching to `handle_page_fault`.
///
/// The values array must have REGS_PER_CPU entries in the order defined above.
pub fn stash(cpu: usize, values: &[u64; REGS_PER_CPU]) {
    if cpu >= MAX_CPUS {
        return;
    }
    for (i, &v) in values.iter().enumerate() {
        DATA[cpu][i].store(v, Ordering::Relaxed);
    }
    VALID[cpu].store(true, Ordering::Release);
}

/// Take the stashed registers for the given CPU. Returns `None` if nothing
/// was stashed. Clears the stash so it won't be read twice.
pub fn take(cpu: usize) -> Option<CrashRegs> {
    if cpu >= MAX_CPUS {
        return None;
    }
    if !VALID[cpu].swap(false, Ordering::Acquire) {
        return None;
    }
    let r = |i: usize| DATA[cpu][i].load(Ordering::Relaxed);
    Some(CrashRegs {
        rax: r(RAX), rbx: r(RBX), rcx: r(RCX), rdx: r(RDX),
        rsi: r(RSI), rdi: r(RDI), rbp: r(RBP), rsp: r(RSP),
        r8: r(R8), r9: r(R9), r10: r(R10), r11: r(R11),
        r12: r(R12), r13: r(R13), r14: r(R14), r15: r(R15),
        rip: r(RIP), rflags: r(RFLAGS), fault_addr: r(FAULT_ADDR),
    })
}
