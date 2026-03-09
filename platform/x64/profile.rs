// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
pub fn read_clock_counter() -> u64 {
    let (tsc, _aux) = unsafe { x86::time::rdtscp() };
    tsc
}
