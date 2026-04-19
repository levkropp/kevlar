// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use core::arch::global_asm;

global_asm!(include_str!("boot.S"));
global_asm!(include_str!("trap.S"));
global_asm!(include_str!("usercopy.S"));
global_asm!(include_str!("usermode.S"));
// AP trampoline uses AT&T syntax (16/32/64-bit mixed code at VMA 0x8000).
global_asm!(include_str!("ap_trampoline.S"), options(att_syntax));

#[macro_use]
mod cpu_local;

mod acpi;
mod apic;
mod backtrace;
mod boot;
mod bootinfo;
mod gdt;
mod idle;
mod idt;
mod interrupt;
mod ioapic;
pub mod paging;
mod pit;
mod profile;
mod semihosting;
mod serial;
pub mod if_trace;
pub mod smp;
mod syscall;
pub mod tsc;
mod tss;
pub mod vdso;
pub mod task;
#[cfg(feature = "ktrace")]
pub mod debugcon;
mod vga;
pub mod fbcon;
pub(crate) mod ps2kbd;
pub mod ps2mouse;

pub use backtrace::Backtrace;
pub use idle::{halt, idle};
pub use interrupt::SavedInterruptStatus;
pub use ioapic::enable_irq;
pub use paging::{PageFaultReason, PageTable, HUGE_PAGE_SIZE, load_kernel_pml4};
pub use profile::read_clock_counter;
pub use semihosting::{semihosting_halt, SemihostingExitStatus};
pub use syscall::PtRegs;

cpu_local! {
    pub static ref CPU_ID: u32 = 0;
}

/// Returns the CPU index for the current CPU (0 = BSP, 1..N = APs).
pub fn cpu_id() -> u32 {
    *CPU_ID.get()
}

/// Start the LAPIC preemption timer on the current AP.
/// Must be called after the per-CPU process state (idle thread, CURRENT)
/// has been fully initialized.
pub fn start_ap_preemption_timer() {
    unsafe { apic::lapic_timer_init(); }
}

/// Log the LAPIC timer registers and heartbeat counters for the current CPU.
/// Safe to call from any context (including with IF=0).
pub fn lapic_timer_diag_log() {
    apic::lapic_timer_diag_log();
}

/// Register the current CPU's APIC ID for NMI watchdog targeting.
pub fn register_cpu_apic_id(cpu_index: u32) {
    apic::register_cpu_apic_id(cpu_index);
}

/// Enable the NMI watchdog (hard lockup detector).
/// Call after all CPUs are online and their LAPIC timers are running.
pub fn watchdog_enable() {
    apic::watchdog_enable();
}

/// Enable the per-CPU data access safety checker.
pub fn enable_preempt_check() {
    cpu_local::enable_preempt_check();
}

/// Assert that per-CPU data access is safe (preemption or interrupts disabled).
/// Called from the cpu_local! macro expansion.
#[inline(always)]
pub fn assert_preempt_safe() {
    cpu_local::assert_preempt_safe();
}

/// Enable the interrupt state tracker.
pub fn if_trace_enable() {
    if_trace::enable();
}

pub fn syscall_counter_read(cpu: usize) -> usize {
    if cpu < syscall::SYSCALL_COUNT.len() {
        syscall::SYSCALL_COUNT[cpu].load(core::sync::atomic::Ordering::Relaxed)
    } else { 0 }
}

pub fn last_syscall_nr_read(cpu: usize) -> u32 {
    if cpu < syscall::LAST_SYSCALL_NR.len() {
        syscall::LAST_SYSCALL_NR[cpu].load(core::sync::atomic::Ordering::Relaxed)
    } else { 0 }
}

/// Dump per-(cpu, syscall_nr) latency histogram to the log.  Safe to
/// call from NMI / panic context.  See `syscall::dump_histogram` for
/// output format.
pub fn syscall_dump_histogram(cpu: usize) {
    syscall::dump_histogram(cpu);
}

/// Periodic watchdog check — called from handle_timer_irq.
pub fn watchdog_check() {
    apic::watchdog_check();
}

/// Lock-free emergency serial write. Safe from any context (signal, NMI, etc).
pub fn emergency_serial_hex(prefix: &[u8], value: u64) {
    serial::emergency_serial_hex(prefix, value);
}

/// Read the CMOS RTC and return seconds since Unix epoch.
pub fn read_rtc_epoch_secs() -> u64 {
    fn cmos_read(reg: u8) -> u8 {
        unsafe {
            x86::io::outb(0x70, reg);
            x86::io::inb(0x71)
        }
    }
    fn bcd_to_bin(bcd: u8) -> u8 {
        (bcd & 0x0F) + (bcd >> 4) * 10
    }

    // Wait until RTC update-in-progress flag is clear.
    while cmos_read(0x0A) & 0x80 != 0 {}

    let sec = bcd_to_bin(cmos_read(0x00)) as u64;
    let min = bcd_to_bin(cmos_read(0x02)) as u64;
    let hour = bcd_to_bin(cmos_read(0x04)) as u64;
    let day = bcd_to_bin(cmos_read(0x07)) as u64;
    let month = bcd_to_bin(cmos_read(0x08)) as u64;
    let year = bcd_to_bin(cmos_read(0x09)) as u64 + 2000;

    // Convert to Unix timestamp (simplified, no leap seconds).
    let mut days: u64 = 0;
    let mut y = 1970;
    while y < year {
        days += if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 366 } else { 365 };
        y += 1;
    }
    let is_leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let mdays: [u64; 12] = [31, if is_leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut m = 0;
    while m < month.saturating_sub(1) as usize && m < 12 {
        days += mdays[m];
        m += 1;
    }
    days += day.saturating_sub(1);

    days * 86400 + hour * 3600 + min * 60 + sec
}

/// Broadcast a "halt immediately" IPI to all CPUs except the current one.
/// Called from the panic handler to freeze other CPUs and prevent interleaved
/// output or double panics on the serial console.
pub fn broadcast_halt_ipi() {
    unsafe { apic::broadcast_halt_ipi(); }
}

pub mod x64_specific {
    pub use super::cpu_local::cpu_local_head;
    pub use super::gdt::{USER_CS32, USER_CS64, USER_DS, USER_RPL};
    pub use super::smp::num_online_cpus;
    pub use super::tss::TSS;
    pub use super::task::{
        ArchTask, switch_task, write_fsbase,
        KERNEL_STACK_SIZE, USER_VALLOC_END, USER_VALLOC_BASE, USER_STACK_TOP,
    };
}

/// Returns the total number of online CPUs (BSP + online APs).
pub fn num_online_cpus() -> u32 {
    smp::num_online_cpus()
}

/// Returns (cpu_family, model, stepping) from CPUID leaf 1.
pub fn cpuid_family_model_stepping() -> (u32, u32, u32) {
    use x86::cpuid::CpuId;
    let info = CpuId::new().get_feature_info().unwrap();
    (info.family_id() as u32, info.model_id() as u32, info.stepping_id() as u32)
}

pub const PAGE_SIZE: usize = 4096;
pub const TICK_HZ: usize = 100;

/// Returns true if hardware interrupts are currently enabled (RFLAGS.IF = 1).
#[inline(always)]
pub fn interrupts_enabled() -> bool {
    use x86::current::rflags;
    rflags::read().contains(rflags::RFlags::FLAGS_IF)
}

/// Enables hardware interrupts (sets RFLAGS.IF = 1).
///
/// Used to re-enable interrupts after entering via an interrupt gate (which
/// clears IF).  Must only be called when the kernel environment is fully set
/// up (register frame saved, per-CPU state accessible).
#[inline(always)]
pub fn enable_interrupts() {
    unsafe { core::arch::asm!("sti", options(nomem, nostack)) }
}

/// Increment the per-CPU preemption disable count.
/// While > 0, the timer preemption handler will not call `process::switch()`.
#[inline(always)]
pub fn preempt_disable() {
    cpu_local::cpu_local_head().preempt_count += 1;
}

/// Decrement the per-CPU preemption disable count.
/// If a reschedule was requested while preemption was disabled (need_resched),
/// trigger it now.  This matches Linux's preempt_check_resched().
#[inline(always)]
pub fn preempt_enable() {
    let head = cpu_local::cpu_local_head();
    head.preempt_count -= 1;
    if head.preempt_count == 0 && head.need_resched != 0 {
        head.need_resched = 0;
        // Call switch via the kernel's process module.  This is a function
        // pointer because the platform crate can't depend on the kernel crate.
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

/// Returns true if preemption is currently disabled (preempt_count > 0).
#[inline(always)]
pub fn in_preempt() -> bool {
    cpu_local::cpu_local_head().preempt_count > 0
}

/// The base virtual address of straight mapping.
pub const KERNEL_BASE_ADDR: usize = 0xffff_8000_0000_0000;

/// The end of straight mapping. Any physical address `P` is mapped into the
/// kernel's virtual memory address `KERNEL_BASE_ADDR + P`.
pub const KERNEL_STRAIGHT_MAP_PADDR_END: usize = 0x1_0000_0000;
