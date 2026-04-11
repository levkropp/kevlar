// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::address::PAddr;
use crate::spinlock::SpinLock;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU32, Ordering};
use x86::msr::{self, rdmsr, wrmsr};

/// The base index of interrupt vectors.
const APIC_BASE_EN: u64 = 1 << 11;
const SIVR_SOFT_EN: u32 = 1 << 8;

// LAPIC register offsets (from LAPIC base 0xfee00000).
const LAPIC_ID_OFF:       usize = 0x020;
const ICR_HIGH_OFF:       usize = 0x310; // destination field
const ICR_LOW_OFF:        usize = 0x300; // command + delivery status
const ICR_PENDING:        u32   = 1 << 12; // Delivery Status bit

// ICR command values.
const ICR_INIT:           u32 = 0x00004500; // INIT IPI: Level=Assert, Mode=INIT
const ICR_SIPI:           u32 = 0x00000600; // STARTUP IPI: Mode=StartUp (vector in [7:0])

// LAPIC timer register offsets.
const LAPIC_LVT_TIMER_OFF:   usize = 0x320;
const LAPIC_INIT_COUNT_OFF:  usize = 0x380;
const LAPIC_CURR_COUNT_OFF:  usize = 0x390;
const LAPIC_DIV_CONF_OFF:    usize = 0x3E0;

const LAPIC_TIMER_MASKED:    u32 = 1 << 16;
const LAPIC_TIMER_PERIODIC:  u32 = 1 << 17;

/// Interrupt vector for per-CPU LAPIC timer preemption (APs).
pub const LAPIC_PREEMPT_VECTOR: u8 = 0x40;

/// Interrupt vector broadcast to all CPUs (except self) to halt them on panic.
pub const PANIC_HALT_VECTOR: u8 = 0x41;

/// Interrupt vector broadcast to all CPUs (except self) for TLB shootdown.
pub const TLB_SHOOTDOWN_VECTOR: u8 = 0x42;

/// Virtual address to invalidate during the current TLB shootdown.
pub static TLB_SHOOTDOWN_VADDR: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

/// Bitmask of CPU indices that have not yet acknowledged the current shootdown.
/// Bit i is set while CPU i has not called invlpg + ack.
pub static TLB_SHOOTDOWN_PENDING: AtomicU32 = AtomicU32::new(0);

/// Serialises concurrent TLB-shootdown senders.
///
/// We use `lock_no_irq` (no cli/sti) deliberately: while CPU A holds this
/// lock and spin-waits for acks, CPU B must be able to receive and handle
/// CPU A's IPI even when CPU B is itself trying to acquire this lock.
/// Interrupt delivery is not suppressed by the spin loop in `lock_no_irq`.
static TLB_SHOOTDOWN_LOCK: crate::spinlock::SpinLock<()> =
    crate::spinlock::SpinLock::new(());

/// Broadcast a "halt now" Fixed IPI to all CPUs except the current one.
///
/// Called from the panic handler after claiming the panic slot, so only the
/// first panicking CPU ever calls this.  Other CPUs receive the IPI on
/// `PANIC_HALT_VECTOR`, disable interrupts, and spin-halt, eliminating
/// double-panic noise and interleaved output on the serial console.
///
/// The "All Excluding Self" shorthand (bits 19:18 = 0b11) avoids the need to
/// know the destination APIC IDs at panic time, which is important because the
/// ACPI/MADT data structures might already be in an inconsistent state.
pub unsafe fn broadcast_halt_ipi() {
    // ICR shorthand 0b11 = All Excluding Self; delivery = Fixed (mode 0).
    const ICR_ALL_EXCL_SELF_FIXED: u32 = (3 << 18) | PANIC_HALT_VECTOR as u32;
    wait_icr_idle();
    lapic_write(ICR_LOW_OFF, ICR_ALL_EXCL_SELF_FIXED);
}

/// Flush a single page from all online CPUs' TLBs.
///
/// Performs the local `invlpg` immediately, then — if there are other online
/// CPUs — broadcasts `TLB_SHOOTDOWN_VECTOR` and spin-waits until every other
/// CPU has acknowledged.
///
/// **Must not be called with hardware interrupts disabled** (i.e. must not be
/// called while holding a regular `SpinLock::lock()` guard).  Callers that
/// hold `lock_no_irq` or `lock_preempt` guards are fine: those do not suppress
/// interrupts, so the receiving CPUs can handle the IPI while spinning.
///
/// Supports up to 32 online CPUs (u32 bitmask).
pub fn tlb_shootdown(vaddr: usize) {
    use core::sync::atomic::Ordering;

    // Local invlpg — always needed and cheap.
    unsafe {
        core::arch::asm!("invlpg [{}]", in(reg) vaddr,
            options(nostack, preserves_flags));
    }

    let num_cpus = super::smp::num_online_cpus();
    if num_cpus <= 1 {
        return;
    }

    let my_cpu = super::cpu_id();
    debug_assert!((my_cpu as u32) < 32, "tlb_shootdown: CPU id >= 32");

    // Bitmask of all CPUs except self.
    let pending: u32 = ((1u32 << num_cpus) - 1) & !(1u32 << my_cpu);

    // Acquire the global shootdown slot (no interrupt disable).
    let _lock = TLB_SHOOTDOWN_LOCK.lock_no_irq();

    // Publish the shootdown address and the pending bitmask.
    // The Release fence ensures both stores are visible to other CPUs
    // before the IPI fires.
    TLB_SHOOTDOWN_VADDR.store(vaddr, Ordering::Relaxed);
    TLB_SHOOTDOWN_PENDING.store(pending, Ordering::Relaxed);
    core::sync::atomic::fence(Ordering::SeqCst);

    // Record the TLB shootdown in the flight recorder.
    crate::flight_recorder::record(
        crate::flight_recorder::kind::TLB_SEND,
        pending,
        vaddr as u64,
        0,
    );

    // Broadcast TLB shootdown IPI to all CPUs except self.
    const ICR_ALL_EXCL_SELF_SHOOTDOWN: u32 = (3 << 18) | TLB_SHOOTDOWN_VECTOR as u32;
    unsafe {
        wait_icr_idle();
        lapic_write(ICR_LOW_OFF, ICR_ALL_EXCL_SELF_SHOOTDOWN);
    }

    // Spin until every target CPU has cleared its bit.
    while TLB_SHOOTDOWN_PENDING.load(Ordering::Acquire) != 0 {
        core::hint::spin_loop();
    }
    // _lock dropped here, releasing the global shootdown slot.
}

/// Send ONE IPI to all other CPUs telling them to reload CR3 (full TLB flush).
///
/// More efficient than calling `tlb_shootdown` per page for large unmaps (e.g.
/// `sys_munmap` over a whole thread stack): reduces N IPI round-trips to 1.
///
/// The receiver sees `TLB_SHOOTDOWN_VADDR == 0` as the sentinel for
/// "reload CR3", vs a non-zero vaddr which means "invlpg that one page".
///
/// Same interrupt-enable requirement as `tlb_shootdown`.
pub fn tlb_remote_full_flush() {
    use core::sync::atomic::Ordering;

    let num_cpus = super::smp::num_online_cpus();
    if num_cpus <= 1 {
        return;
    }

    let my_cpu = super::cpu_id();
    debug_assert!((my_cpu as u32) < 32, "tlb_remote_full_flush: CPU id >= 32");

    let pending: u32 = ((1u32 << num_cpus) - 1) & !(1u32 << my_cpu);

    let _lock = TLB_SHOOTDOWN_LOCK.lock_no_irq();

    // vaddr = 0 is the "full flush" sentinel; receiver reloads CR3.
    TLB_SHOOTDOWN_VADDR.store(0, Ordering::Relaxed);
    TLB_SHOOTDOWN_PENDING.store(pending, Ordering::Relaxed);
    core::sync::atomic::fence(Ordering::SeqCst);

    // Record the full-TLB shootdown (vaddr=0 sentinel) in the flight recorder.
    crate::flight_recorder::record(
        crate::flight_recorder::kind::TLB_SEND,
        pending,
        0, // vaddr=0 means full CR3 reload
        0,
    );

    const ICR_ALL_EXCL_SELF_SHOOTDOWN: u32 = (3 << 18) | TLB_SHOOTDOWN_VECTOR as u32;
    unsafe {
        wait_icr_idle();
        lapic_write(ICR_LOW_OFF, ICR_ALL_EXCL_SELF_SHOOTDOWN);
    }

    while TLB_SHOOTDOWN_PENDING.load(Ordering::Acquire) != 0 {
        core::hint::spin_loop();
    }
}

/// LAPIC timer ticks in 10ms, measured by the BSP during calibration.
static LAPIC_TICKS_PER_10MS: AtomicU32 = AtomicU32::new(0);

static APIC: SpinLock<LocalApic> = SpinLock::new(LocalApic::new(PAddr::new(0xfee0_0000)));

#[derive(Debug, Copy, Clone)]
#[repr(u32)]
enum LocalApicReg {
    #[allow(dead_code)]
    Eoi = 0xb0,
    SpuriousInterrupt = 0xf0,
}

struct LocalApic {
    base: PAddr,
}

impl LocalApic {
    pub const fn new(base: PAddr) -> LocalApic {
        LocalApic { base }
    }

    #[allow(dead_code)]
    pub unsafe fn write_eoi(&self) {
        // The EOI register accepts only 0. CPU raises #GP otherwise.
        self.mmio_write(LocalApicReg::Eoi, 0);
    }

    pub unsafe fn write_spurious_interrupt(&self, value: u32) {
        self.mmio_write(LocalApicReg::SpuriousInterrupt, value);
    }

    #[inline(always)]
    unsafe fn _mmio_read(&self, reg: LocalApicReg) -> u32 {
        read_volatile(self.base.add(reg as usize).as_ptr())
    }

    #[inline(always)]
    unsafe fn mmio_write(&self, reg: LocalApicReg, value: u32) {
        write_volatile(self.base.add(reg as usize).as_mut_ptr(), value)
    }
}

/// Acknowledge the current interrupt.
///
/// This is called from the interrupt handler on every IRQ.  We write the
/// EOI register directly instead of going through the SpinLock — this
/// kernel is single-CPU and the interrupt handler runs with interrupts
/// disabled, so the lock acquire/release was pure overhead (cli/sti +
/// deadlock check + backtrace capture in debug builds).
#[inline(always)]
pub fn ack_interrupt() {
    unsafe {
        // Safety: single-CPU, interrupts disabled in this context.
        // The APIC base (0xfee00000) is a hardware-fixed address that
        // never changes.  EOI register is at offset 0xb0.
        let eoi_addr = PAddr::new(0xfee0_00b0).as_vaddr();
        core::ptr::write_volatile(eoi_addr.as_mut_ptr::<u32>(), 0);
    }
}

pub unsafe fn init() {
    // Activate Local APIC.
    let apic_base = rdmsr(msr::APIC_BASE);
    wrmsr(msr::APIC_BASE, apic_base | APIC_BASE_EN);

    let apic = APIC.lock();
    apic.write_spurious_interrupt(SIVR_SOFT_EN);
}

// ── Direct LAPIC MMIO helpers (no lock — used during early SMP boot) ──

/// Returns the APIC ID of the current CPU (bits [31:24] of LAPIC ID reg).
pub unsafe fn lapic_id() -> u8 {
    let val = lapic_read(LAPIC_ID_OFF);
    (val >> 24) as u8
}

/// Send an INIT IPI to the AP with the given APIC ID.
pub unsafe fn send_init_ipi(apic_id: u8) {
    lapic_write(ICR_HIGH_OFF, (apic_id as u32) << 24);
    lapic_write(ICR_LOW_OFF, ICR_INIT);
    wait_icr_idle();
}

/// Send a STARTUP IPI to the AP with the given APIC ID.
/// `vector` is the trampoline page number (physical_addr >> 12), e.g. 8 for 0x8000.
pub unsafe fn send_sipi(apic_id: u8, vector: u8) {
    lapic_write(ICR_HIGH_OFF, (apic_id as u32) << 24);
    lapic_write(ICR_LOW_OFF, ICR_SIPI | vector as u32);
    wait_icr_idle();
}

/// Spin until the ICR Delivery Status bit clears (IPI has been accepted).
unsafe fn wait_icr_idle() {
    while lapic_read(ICR_LOW_OFF) & ICR_PENDING != 0 {
        core::hint::spin_loop();
    }
}

/// Calibrate the LAPIC timer by counting ticks over 10 ms using the TSC.
/// Called once by the BSP after `tsc::calibrate()`.  The result is stored
/// in `LAPIC_TICKS_PER_10MS` and used by every CPU calling `lapic_timer_init`.
pub unsafe fn lapic_timer_calibrate() {
    // Divide-by-1, one-shot, masked — we just want to count.
    lapic_write(LAPIC_DIV_CONF_OFF, 0xB);
    lapic_write(LAPIC_LVT_TIMER_OFF, LAPIC_TIMER_MASKED | LAPIC_PREEMPT_VECTOR as u32);
    lapic_write(LAPIC_INIT_COUNT_OFF, u32::MAX);

    let start = super::tsc::nanoseconds_since_boot();
    while super::tsc::nanoseconds_since_boot() - start < 10_000_000 {}

    let remaining = lapic_read(LAPIC_CURR_COUNT_OFF);
    lapic_write(LAPIC_INIT_COUNT_OFF, 0); // stop the timer

    let ticks_10ms = u32::MAX.wrapping_sub(remaining);
    LAPIC_TICKS_PER_10MS.store(ticks_10ms, Ordering::Relaxed);
    trace!("apic: LAPIC timer ~{} ticks/10ms", ticks_10ms);
}

/// Configure and start the LAPIC timer on the current CPU in periodic mode.
/// Fires every ~10 ms (100 Hz) on vector `LAPIC_PREEMPT_VECTOR`.
/// Requires `lapic_timer_calibrate()` to have been called by the BSP first.
pub unsafe fn lapic_timer_init() {
    let ticks_per_10ms = LAPIC_TICKS_PER_10MS.load(Ordering::Relaxed);
    if ticks_per_10ms == 0 {
        warn!("apic: LAPIC timer not calibrated, skipping");
        return;
    }
    let cpu = super::cpu_id();
    log::trace!("apic: starting LAPIC timer on cpu {} (ticks_per_10ms={})", cpu, ticks_per_10ms);
    lapic_write(LAPIC_DIV_CONF_OFF, 0xB);
    lapic_write(LAPIC_LVT_TIMER_OFF, LAPIC_TIMER_PERIODIC | LAPIC_PREEMPT_VECTOR as u32);
    lapic_write(LAPIC_INIT_COUNT_OFF, ticks_per_10ms);
}

#[inline(always)]
unsafe fn lapic_read(offset: usize) -> u32 {
    read_volatile(PAddr::new(0xfee0_0000 + offset).as_vaddr().as_ptr::<u32>())
}

#[inline(always)]
unsafe fn lapic_write(offset: usize, value: u32) {
    write_volatile(
        PAddr::new(0xfee0_0000 + offset).as_vaddr().as_mut_ptr::<u32>(),
        value,
    );
}
