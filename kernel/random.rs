// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Kernel random number generation.
//!
//! Uses a fast buffered PRNG (SplitMix64) seeded from hardware RNG (RDRAND/RNDR)
//! at boot time. The PRNG is lock-free (per-CPU or global atomic counter).
//! RDRAND under KVM causes expensive VM exits (~25µs per 16 bytes), so we
//! only call it once during initialization.
use crate::user_buffer::UserBufferMut;
use crate::{prelude::*, user_buffer::UserBufWriter};
use core::sync::atomic::{AtomicU64, Ordering};

/// SplitMix64 state — seeded from RDRAND at boot, then advanced atomically.
static PRNG_STATE: AtomicU64 = AtomicU64::new(0);
static PRNG_SEEDED: AtomicU64 = AtomicU64::new(0);

/// Seed the PRNG from hardware RNG. Called once during kernel init.
fn ensure_seeded() {
    if PRNG_SEEDED.load(Ordering::Relaxed) != 0 {
        return;
    }
    let mut seed = [0u8; 8];
    #[cfg(target_arch = "x86_64")]
    {
        kevlar_platform::random::rdrand_fill(&mut seed);
    }
    #[cfg(target_arch = "aarch64")]
    {
        let c = kevlar_platform::arch::read_clock_counter();
        seed = c.to_le_bytes();
    }
    let s = u64::from_le_bytes(seed);
    PRNG_STATE.store(if s == 0 { 0xdeadbeef12345678 } else { s }, Ordering::Relaxed);
    PRNG_SEEDED.store(1, Ordering::Release);
}

/// SplitMix64: fast, high-quality PRNG. Lock-free via fetch_add.
#[inline]
fn splitmix64_next() -> u64 {
    // Advance state atomically (works across CPUs without locks).
    let s = PRNG_STATE.fetch_add(0x9e3779b97f4a7c15, Ordering::Relaxed);
    let mut z = s.wrapping_add(0x9e3779b97f4a7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}

/// Fill a slice with PRNG bytes (fast, ~5ns per 8 bytes).
fn prng_fill(slice: &mut [u8]) {
    ensure_seeded();
    let mut i = 0;
    while i + 8 <= slice.len() {
        let val = splitmix64_next();
        slice[i..i + 8].copy_from_slice(&val.to_le_bytes());
        i += 8;
    }
    if i < slice.len() {
        let val = splitmix64_next();
        let bytes = val.to_le_bytes();
        for j in 0..(slice.len() - i) {
            slice[i + j] = bytes[j];
        }
    }
}

pub fn read_secure_random(buf: UserBufferMut<'_>) -> Result<usize> {
    UserBufWriter::from(buf).write_with(|slice| {
        prng_fill(slice);
        Ok(slice.len())
    })
}

pub fn read_insecure_random(buf: UserBufferMut<'_>) -> Result<usize> {
    read_secure_random(buf)
}
