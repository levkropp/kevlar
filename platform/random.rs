// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Safe wrappers for hardware random number generation.

/// Fill a slice with random bytes using RDRAND (x86_64).
/// Returns true if the data is valid.
#[cfg(target_arch = "x86_64")]
pub fn rdrand_fill(slice: &mut [u8]) -> bool {
    unsafe { x86::random::rdrand_slice(slice) }
}
