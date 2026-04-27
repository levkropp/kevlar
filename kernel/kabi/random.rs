// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Random-byte source shims.
//!
//! K11: deterministic non-zero pattern (good enough for
//! `get_random_bytes` calls inside `init_module`, which typically
//! seed MAC-address fields and per-cpu hashes that we don't
//! observe).  K12+ wires real entropy when a driver actually
//! depends on randomness.

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn get_random_bytes(buf: *mut u8, n: usize) {
    if buf.is_null() {
        return;
    }
    for i in 0..n {
        unsafe { *buf.add(i) = ((i ^ 0xa5).wrapping_mul(31)) as u8 };
    }
}

ksym!(get_random_bytes);
