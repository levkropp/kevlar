// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
pub fn read_clock_counter() -> u64 {
    super::timer::counter()
}

/// Frequency (Hz) of `read_clock_counter`.  On arm64 this is `CNTFRQ_EL0` —
/// typically 24 MHz on HVF/KVM, 62.5 MHz on native Apple Silicon.
pub fn read_clock_frequency() -> u64 {
    super::timer::cntfrq_public()
}
