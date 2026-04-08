// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use core::arch::global_asm;

global_asm!(include_str!("boot.S"));
global_asm!(include_str!("trap.S"));
global_asm!(include_str!("usercopy.S"));
global_asm!(include_str!("usermode.S"));

#[macro_use]
mod cpu_local;

mod backtrace;
mod boot;
mod bootinfo;
#[cfg(feature = "ktrace")]
pub mod debugcon;
mod gic;
mod idle;
mod interrupt;
mod paging;
mod profile;
mod semihosting;
mod serial;
pub mod smp;
mod syscall;
pub mod task;
mod timer;

pub use backtrace::Backtrace;
pub use idle::{halt, idle};
pub use interrupt::SavedInterruptStatus;
pub use gic::enable_irq;
pub use paging::{PageFaultReason, PageTable};
pub use profile::read_clock_counter;
pub use semihosting::{semihosting_halt, SemihostingExitStatus};
pub use syscall::PtRegs;

pub mod arm64_specific {
    pub use super::cpu_local::cpu_local_head;
    pub use super::task::{
        ArchTask, switch_task, write_tls_base,
        KERNEL_STACK_SIZE, USER_VALLOC_END, USER_VALLOC_BASE, USER_STACK_TOP,
    };
}

/// Per-CPU ID (0 = BSP, 1..N = APs in startup order).
cpu_local! {
    pub static ref CPU_ID: u32 = 0;
}

/// Returns the index of the calling CPU (0 = BSP).
pub fn cpu_id() -> u32 {
    *CPU_ID.get()
}

/// Returns the total number of online CPUs.
pub fn num_online_cpus() -> u32 {
    smp::num_online_cpus()
}

/// Start the per-AP preemption timer.
/// Called from `ap_kernel_entry` after `process::init_ap()` sets up CURRENT.
pub fn start_ap_preemption_timer() {
    unsafe { timer::init_ap() }
}

/// Broadcast a halt IPI to all other CPUs on panic.
/// TODO: implement via GIC SGI (Software Generated Interrupt).
pub fn broadcast_halt_ipi() {
    // ARM64/GIC implementation pending. Other CPUs will eventually see
    // PANICKED=true and halt on their next interrupt.
}

/// Read RTC epoch seconds. ARM64 has no CMOS; returns 0 (boot-relative).
pub fn read_rtc_epoch_secs() -> u64 {
    0
}

pub const PAGE_SIZE: usize = 4096;
pub const HUGE_PAGE_SIZE: usize = 512 * PAGE_SIZE; // 2MB with 4KB granule (stub)
pub const TICK_HZ: usize = 50;

/// Returns true if hardware interrupts are currently enabled (DAIF.I = 0).
#[inline(always)]
pub fn interrupts_enabled() -> bool {
    let daif: u64;
    unsafe { core::arch::asm!("mrs {}, daif", out(reg) daif) }
    // DAIF bit 7 = I (IRQ mask). I=0 means interrupts are enabled.
    daif & (1 << 7) == 0
}

/// Enables hardware interrupts (clears DAIF.I).
///
/// Used to re-enable interrupts after entering via an exception vector that
/// masked IRQs.  Must only be called when the kernel environment is fully
/// set up (register frame saved, per-CPU state accessible).
#[inline(always)]
pub fn enable_interrupts() {
    unsafe { core::arch::asm!("msr daifclr, #2", options(nomem, nostack)) }
}

/// Increment the per-CPU preemption disable count.
/// While > 0, the timer preemption handler will not call `process::switch()`.
#[inline(always)]
pub fn preempt_disable() {
    cpu_local::cpu_local_head().preempt_count += 1;
}

/// Decrement the per-CPU preemption disable count.
#[inline(always)]
pub fn preempt_enable() {
    cpu_local::cpu_local_head().preempt_count -= 1;
}

/// Returns true if preemption is currently disabled (preempt_count > 0).
#[inline(always)]
pub fn in_preempt() -> bool {
    cpu_local::cpu_local_head().preempt_count > 0
}

/// The base virtual address of straight mapping (TTBR1 region).
pub const KERNEL_BASE_ADDR: usize = 0xffff_0000_0000_0000;

/// The end of straight mapping. Any physical address `P` is mapped into the
/// kernel's virtual memory address `KERNEL_BASE_ADDR + P`.
pub const KERNEL_STRAIGHT_MAP_PADDR_END: usize = 0x1_0000_0000;
