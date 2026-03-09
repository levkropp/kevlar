// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
pub fn read_clock_counter() -> u64 {
    super::timer::counter()
}
