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
pub use profile::{read_clock_counter, read_clock_frequency};
pub use timer::nanoseconds_since_boot;
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

/// Read RTC epoch seconds from the QEMU virt PL031 RTC.
///
/// PL031 lives at paddr 0x09010000 (QEMU virt board).  Its data register
/// (offset 0x00) returns a 32-bit count of seconds since the Unix epoch
/// (QEMU initialises it from the host clock at VM startup).  Low paddrs are
/// mapped by boot.S as Device memory (AttrIndx=0), so we access it through
/// the straight kernel map.  Returns 0 if the RTC reads back zero (no host
/// clock seed — e.g. semihosting-only boot).
pub fn read_rtc_epoch_secs() -> u64 {
    const PL031_DR_PADDR: usize = 0x09010000;
    let vaddr = KERNEL_BASE_ADDR + PL031_DR_PADDR;
    let secs: u32 = unsafe { core::ptr::read_volatile(vaddr as *const u32) };
    secs as u64
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
/// If a reschedule was requested while preemption was disabled (need_resched),
/// trigger it now.  Mirrors x86's preempt_enable → preempt_check_resched.
#[inline(always)]
pub fn preempt_enable() {
    let head = cpu_local::cpu_local_head();
    head.preempt_count -= 1;
    if head.preempt_count == 0 && head.need_resched != 0 {
        head.need_resched = 0;
        let f = RESCHED_FN.load(core::sync::atomic::Ordering::Relaxed);
        if !f.is_null() {
            let switch_fn: fn() -> bool = unsafe { core::mem::transmute(f) };
            switch_fn();
        }
    }
}

/// Function pointer for deferred rescheduling from `preempt_enable`.
/// Set by the kernel during init to `process::switch`.
static RESCHED_FN: core::sync::atomic::AtomicPtr<()> =
    core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());

/// Register the reschedule function (called from kernel init).
pub fn set_resched_fn(f: fn() -> bool) {
    RESCHED_FN.store(f as *mut (), core::sync::atomic::Ordering::Relaxed);
}

/// Mark that a reschedule is needed on this CPU (called from timer handler
/// when preemption is disabled).
#[inline(always)]
pub fn set_need_resched() {
    cpu_local::cpu_local_head().need_resched = 1;
}

/// Register the current CPU's APIC ID for NMI watchdog targeting.
/// ARM64 equivalent would be MPIDR_EL1-based GIC affinity routing — not yet
/// implemented. No-op stub keeps the cross-arch kernel init path working.
pub fn register_cpu_apic_id(_cpu_index: u32) {}

/// Enable the NMI watchdog (hard lockup detector).
/// ARM64: would need GIC-based non-maskable interrupt support (GICv3 NMI /
/// FIQ-based watchdog). No-op stub until ported.
pub fn watchdog_enable() {}

/// Periodic watchdog check — called from handle_timer_irq.
/// No-op until ARM64 GIC NMI support is ported.
pub fn watchdog_check() {}

/// Enable the interrupt state tracker.
/// ARM64: would track DAIF.I transitions. No-op until ported.
pub fn if_trace_enable() {}

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
