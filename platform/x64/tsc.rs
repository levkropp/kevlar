// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! TSC (Time Stamp Counter) calibration and nanosecond-resolution timing.
//!
//! Calibrates the TSC frequency against the PIT (Programmable Interval Timer)
//! during early boot, then provides lock-free nanosecond-resolution reads
//! via `rdtscp`.
//!
//! The calibration works by programming the PIT for a known delay and
//! measuring how many TSC ticks elapse.  On KVM the TSC is constant-rate
//! and this gives good results (~0.1% accuracy).

use core::sync::atomic::{AtomicU64, Ordering};
use x86::io::{inb, outb};

/// TSC ticks per second, set once during calibration.
static TSC_FREQ_HZ: AtomicU64 = AtomicU64::new(0);

/// TSC value at the moment calibration completed (our time origin).
static TSC_ORIGIN: AtomicU64 = AtomicU64::new(0);

/// Precomputed fixed-point multiplier: (10^9 << 32) / freq.
/// Allows converting TSC delta → nanoseconds via a single u128 multiply
/// instead of two u64 divisions.
static NS_MULT: AtomicU64 = AtomicU64::new(0);

/// Calibrate the TSC against the PIT.
///
/// Uses PIT channel 2 in one-shot mode to measure a ~10 ms window.
/// Must be called with interrupts disabled (before PIT channel 0 is
/// configured for timer IRQs).
pub unsafe fn calibrate() {
    // PIT oscillator frequency: 1,193,182 Hz.
    const PIT_HZ: u64 = 1_193_182;

    // Target ~10 ms calibration window.  Longer = more accurate but slower boot.
    const TARGET_MS: u64 = 10;
    let pit_count: u16 = ((PIT_HZ * TARGET_MS) / 1000) as u16;

    // Program PIT channel 2 in mode 0 (interrupt on terminal count).
    // Bit 0 of port 0x61 gates the channel 2 counter.
    let gate = inb(0x61);
    // Disable speaker output (bit 1), enable gate (bit 0).
    outb(0x61, (gate & !0x02) | 0x01);

    // Channel 2, lobyte/hibyte, mode 0, binary.
    outb(0x43, 0b1011_0000);
    outb(0x42, (pit_count & 0xff) as u8);
    outb(0x42, (pit_count >> 8) as u8);

    // Read the TSC at the start.
    let tsc_start = x86::time::rdtscp().0;

    // Wait for the PIT to count down.  Channel 2 output (bit 5 of port 0x61)
    // goes high when the count reaches zero.
    while inb(0x61) & 0x20 == 0 {
        core::hint::spin_loop();
    }

    let tsc_end = x86::time::rdtscp().0;

    // Restore the gate register.
    outb(0x61, gate);

    let tsc_delta = tsc_end - tsc_start;
    // Compute frequency: tsc_delta ticks in (pit_count / PIT_HZ) seconds.
    // freq = tsc_delta * PIT_HZ / pit_count
    let freq = tsc_delta * PIT_HZ / pit_count as u64;

    // Precompute fixed-point multiplier: (10^9 << 32) / freq.
    // At 3 GHz this is ~1,431,655,765 — fits easily in u64.
    let mult = (1_000_000_000u128 << 32) / freq as u128;

    TSC_FREQ_HZ.store(freq, Ordering::Release);
    NS_MULT.store(mult as u64, Ordering::Release);
    TSC_ORIGIN.store(tsc_end, Ordering::Release);

    info!("tsc: calibrated frequency = {} MHz", freq / 1_000_000);
}

/// Returns `true` if the TSC has been calibrated.
#[inline]
pub fn is_calibrated() -> bool {
    TSC_FREQ_HZ.load(Ordering::Relaxed) != 0
}

/// Read the current TSC-based time as nanoseconds since boot.
///
/// Returns 0 if the TSC has not been calibrated yet.
#[inline]
pub fn nanoseconds_since_boot() -> u64 {
    let mult = NS_MULT.load(Ordering::Relaxed);
    if mult == 0 {
        return 0;
    }

    let origin = TSC_ORIGIN.load(Ordering::Relaxed);
    let now = unsafe { x86::time::rdtscp().0 };
    let delta = now.wrapping_sub(origin);

    // Convert TSC ticks to nanoseconds via fixed-point multiply:
    //   ns = (delta * mult) >> 32
    //
    // This replaces two u64 divisions (~60-160 cycles) with one u128
    // multiply (~6 cycles).  Precision is <1 ppb for typical TSC
    // frequencies.
    ((delta as u128 * mult as u128) >> 32) as u64
}

/// Return the calibrated TSC frequency in Hz.
#[inline]
pub fn frequency_hz() -> u64 {
    TSC_FREQ_HZ.load(Ordering::Relaxed)
}

/// Return the TSC origin (value at calibration time).
#[inline]
pub fn tsc_origin() -> u64 {
    TSC_ORIGIN.load(Ordering::Relaxed)
}

/// Return the precomputed fixed-point multiplier for TSC→ns conversion.
#[inline]
pub fn ns_mult() -> u64 {
    NS_MULT.load(Ordering::Relaxed)
}
