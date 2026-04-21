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
pub mod lockdep;
pub mod usercopy_trace;
pub mod page_trace;

/// ktrace trace-device output — ISA debugcon on x86_64, semihosting on ARM64.
///
/// Both transports accept a QEMU chardev that writes to `ktrace.bin`:
/// - x86_64: `-chardev file,id=ktrace,path=ktrace.bin -device isa-debugcon,chardev=ktrace,iobase=0xe9`
/// - arm64:  `-chardev file,id=ktrace,path=ktrace.bin -semihosting-config enable=on,target=native,chardev=ktrace`
#[cfg(feature = "ktrace")]
pub mod debugcon {
    /// Write bytes to the architecture trace device.
    ///
    /// One call per ring-buffer dump; implementations batch as much as
    /// possible — a loop of `outb` on x86_64, a single `SYS_WRITE` trap
    /// on ARM64.
    pub fn write_bytes(data: &[u8]) {
        #[cfg(target_arch = "x86_64")]
        crate::x64::debugcon::write_bytes(data);
        #[cfg(target_arch = "aarch64")]
        crate::arm64::debugcon::write_bytes(data);
    }
}

#[cfg(target_arch = "x86_64")]
mod x64;
#[cfg(target_arch = "aarch64")]
mod arm64;

pub mod arch {
    #[cfg(target_arch = "x86_64")]
    pub use super::x64::{
        broadcast_halt_ipi, cpu_id, cpuid_family_model_stepping, emergency_serial_hex,
        enable_irq, halt, idle,
        enable_interrupts, in_preempt, interrupts_enabled,
        num_online_cpus, preempt_disable, preempt_enable, set_need_resched, set_resched_fn,
        read_clock_counter, read_rtc_epoch_secs,
        semihosting_halt,
        start_ap_preemption_timer, lapic_timer_diag_log,
        register_cpu_apic_id, watchdog_enable, watchdog_check,
        if_trace_enable, enable_preempt_check, assert_preempt_safe,
        syscall_counter_read, last_syscall_nr_read, syscall_dump_histogram,
        read_all_qsc_counters, wait_for_qsc_grace_period, QscSnapshot,
        x64_specific, tsc, vdso,
        Backtrace, PageFaultReason, PageTable, PtRegs, SavedInterruptStatus, SemihostingExitStatus,
        KERNEL_BASE_ADDR, KERNEL_STRAIGHT_MAP_PADDR_END, PAGE_SIZE, HUGE_PAGE_SIZE, TICK_HZ,
        fbcon, ps2mouse,
    };

    /// Bump the global PCID generation. Forces every CPU to do a full TLB
    /// flush of its current PCID on the next context switch. Used as a
    /// deferred TLB invalidate when sending an IPI is unsafe (IF=0).
    /// x86_64 only — ARM64 has no PCID equivalent so this is a no-op.
    #[cfg(target_arch = "x86_64")]
    pub fn bump_global_pcid_generation() {
        super::x64::paging::bump_global_pcid_generation();
    }
    #[cfg(target_arch = "aarch64")]
    pub fn bump_global_pcid_generation() {}

    /// Load the kernel bootstrap PML4 into CR3.  Called by the scheduler
    /// when switching to a task that has no Vm (typically the idle thread):
    /// without this, CR3 stays pointing at the outgoing task's pml4, and a
    /// page-walk on that CPU can still traverse that address space even
    /// after its owner exits and tears it down — corrupting freed PT pages
    /// via A/D-bit updates.  ARM64 has a similar concept (TTBR0_EL1) but
    /// we're not wiring it here yet; leave a no-op stub.
    #[cfg(target_arch = "x86_64")]
    pub fn load_kernel_page_table() {
        super::x64::load_kernel_pml4();
    }
    #[cfg(target_arch = "aarch64")]
    pub fn load_kernel_page_table() {}

    #[cfg(target_arch = "aarch64")]
    pub fn syscall_counter_read(_cpu: usize) -> usize { 0 }
    #[cfg(target_arch = "aarch64")]
    pub fn last_syscall_nr_read(_cpu: usize) -> u32 { 0 }
    #[cfg(target_arch = "aarch64")]
    pub fn syscall_dump_histogram(_cpu: usize) {}

    // QSC grace-period is x86_64-only; ARM64 uses different (simpler)
    // TLB invalidation semantics and hasn't hit the same walker-race class.
    #[cfg(target_arch = "aarch64")]
    pub type QscSnapshot = [u64; 8];
    #[cfg(target_arch = "aarch64")]
    pub fn read_all_qsc_counters() -> QscSnapshot { [0; 8] }
    #[cfg(target_arch = "aarch64")]
    pub fn wait_for_qsc_grace_period(_snapshot: &QscSnapshot) -> Result<(), ()> { Ok(()) }

    #[cfg(target_arch = "aarch64")]
    pub use super::arm64::{
        broadcast_halt_ipi, cpu_id, enable_interrupts, enable_irq, halt, idle, in_preempt, interrupts_enabled,
        num_online_cpus, preempt_disable, preempt_enable, read_clock_counter, read_rtc_epoch_secs,
        semihosting_halt,
        start_ap_preemption_timer, arm64_specific, Backtrace,
        PageFaultReason, PageTable, PtRegs, SavedInterruptStatus, SemihostingExitStatus,
        KERNEL_BASE_ADDR, KERNEL_STRAIGHT_MAP_PADDR_END, PAGE_SIZE, HUGE_PAGE_SIZE, TICK_HZ,
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

    /// Called when a complete PS/2 mouse packet is received.
    /// Wakes any process waiting on /dev/input/mice.
    fn handle_mouse_event(&self) {}

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
