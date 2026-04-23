// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
pub fn read_clock_counter() -> u64 {
    let (tsc, _aux) = unsafe { x86::time::rdtscp() };
    tsc
}

/// Frequency (Hz) of `read_clock_counter`.  Delegates to the TSC calibration.
pub fn read_clock_frequency() -> u64 {
    super::tsc::frequency_hz()
}
