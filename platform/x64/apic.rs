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

/// LAPIC timer ticks in 10ms, measured by the BSP during calibration.
static LAPIC_TICKS_PER_10MS: AtomicU32 = AtomicU32::new(0);

static APIC: SpinLock<LocalApic> = SpinLock::new(LocalApic::new(PAddr::new(0xfee0_0000)));

#[derive(Debug, Copy, Clone)]
#[repr(u32)]
enum LocalApicReg {
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
