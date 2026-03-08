// SPDX-License-Identifier: MIT OR Apache-2.0
pub fn read_clock_counter() -> u64 {
    let (tsc, _aux) = unsafe { x86::time::rdtscp() };
    tsc
}
