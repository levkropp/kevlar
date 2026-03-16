// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! An OS-agnostic bootstrap and runtime support library for operating system
//! kernels.
#![no_std]
#![allow(unsafe_op_in_unsafe_fn)]

// Ensure exactly one safety profile is active.
#[cfg(not(any(
    feature = "profile-fortress",
    feature = "profile-balanced",
    feature = "profile-performance",
    feature = "profile-ludicrous",
)))]
compile_error!("Exactly one safety profile must be enabled. Add one of: profile-fortress, profile-balanced, profile-performance, profile-ludicrous");

extern crate alloc;

#[macro_use]
extern crate log;

#[macro_use]
pub mod print;

pub mod address;
pub mod backtrace;
pub mod capabilities;
mod mem;
pub mod page_ops;
pub mod pod;
pub mod random;
pub mod sync;
pub mod bootinfo;
pub mod global_allocator;
pub mod logger;
pub mod crash_regs;
pub mod flight_recorder;
pub mod page_allocator;
pub mod page_refcount;
pub mod stack_cache;
pub mod profile;
pub mod spinlock;
pub mod usercopy_trace;

#[cfg(target_arch = "x86_64")]
mod x64;
#[cfg(target_arch = "aarch64")]
mod arm64;

pub mod arch {
    #[cfg(target_arch = "x86_64")]
    pub use super::x64::{
        broadcast_halt_ipi, cpu_id, cpuid_family_model_stepping, enable_irq, halt, idle,
        in_preempt, interrupts_enabled,
        num_online_cpus, preempt_disable, preempt_enable, read_clock_counter, semihosting_halt,
        start_ap_preemption_timer, x64_specific, tsc, vdso,
        Backtrace, PageFaultReason, PageTable, PtRegs, SavedInterruptStatus, SemihostingExitStatus,
        KERNEL_BASE_ADDR, KERNEL_STRAIGHT_MAP_PADDR_END, PAGE_SIZE, HUGE_PAGE_SIZE, TICK_HZ,
    };

    #[cfg(target_arch = "aarch64")]
    pub use super::arm64::{
        broadcast_halt_ipi, cpu_id, enable_irq, halt, idle, in_preempt, interrupts_enabled,
        num_online_cpus, preempt_disable, preempt_enable, read_clock_counter, semihosting_halt,
        start_ap_preemption_timer, arm64_specific, Backtrace,
        PageFaultReason, PageTable, PtRegs, SavedInterruptStatus, SemihostingExitStatus,
        KERNEL_BASE_ADDR, KERNEL_STRAIGHT_MAP_PADDR_END, PAGE_SIZE, TICK_HZ,
    };
}

use address::UserVAddr;
use kevlar_utils::static_cell::StaticCell;

pub trait Handler: Sync {
    fn handle_console_rx(&self, char: u8);
    fn handle_irq(&self, irq: u8);
    /// Returns `true` if a context switch occurred during the timer tick.
    /// The interrupt handler uses this to skip signal delivery via the old
    /// thread's frame — the new thread gets signals on its next preemption.
    fn handle_timer_irq(&self) -> bool;
    fn handle_page_fault(
        &self,
        unaligned_vaddr: Option<UserVAddr>,
        ip: usize,
        _reason: arch::PageFaultReason,
    );

    /// Called when a non-page-fault exception (GPF, invalid opcode, etc.)
    /// occurs in userspace.  The kernel should deliver a fatal signal.
    fn handle_user_fault(&self, exception: &str, ip: usize);

    #[allow(clippy::too_many_arguments)]
    fn handle_syscall(
        &self,
        a1: usize,
        a2: usize,
        a3: usize,
        a4: usize,
        a5: usize,
        a6: usize,
        n: usize,
        frame: *mut arch::PtRegs,
    ) -> isize;

    /// Called on every LAPIC timer tick on an AP to trigger preemption.
    /// Returns `true` if a context switch actually occurred.
    /// Default implementation is a no-op (safe if called before kernel is ready).
    fn handle_ap_preempt(&self) -> bool { false }

    /// Fast check: returns the signal_pending bitmask for the current process.
    /// Used to skip PtRegs construction on interrupt return when no signals
    /// are pending (the common case for page faults).
    fn current_process_signal_pending(&self) -> u32 { 0 }

    /// Called when an interrupt is about to return to user space.
    /// The kernel may modify the frame to deliver a pending signal.
    /// Default implementation is a no-op.
    fn handle_interrupt_return(&self, _frame: *mut arch::PtRegs) {}

    #[cfg(debug_assertions)]
    fn usercopy_hook(&self) {}
}

static HANDLER: StaticCell<&dyn Handler> = StaticCell::new(&NopHandler);

struct NopHandler;

impl Handler for NopHandler {
    fn handle_console_rx(&self, _char: u8) {}
    fn handle_irq(&self, _irq: u8) {}
    fn handle_timer_irq(&self) -> bool { false }

    fn handle_page_fault(
        &self,
        _unaligned_vaddr: Option<UserVAddr>,
        _ip: usize,
        _reason: arch::PageFaultReason,
    ) {
    }

    fn handle_user_fault(&self, _exception: &str, _ip: usize) {}

    fn handle_syscall(
        &self,
        _a1: usize,
        _a2: usize,
        _a3: usize,
        _a4: usize,
        _a5: usize,
        _a6: usize,
        _n: usize,
        _frame: *mut arch::PtRegs,
    ) -> isize {
        0
    }
}

fn handler() -> &'static dyn Handler {
    HANDLER.load()
}

pub fn set_handler(handler: &'static dyn Handler) {
    HANDLER.store(handler);
}
