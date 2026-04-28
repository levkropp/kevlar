// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Software-fallback bit operations (`__sw_hweight*`).
//!
//! Linux exports `__sw_hweight32/64` so generic code can call them
//! when the arch lacks a hardware popcount.  aarch64 has `cnt`, but
//! some CONFIG paths still emit references to the software variant.
//! We just dispatch to the Rust intrinsic.

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn __sw_hweight64(w: u64) -> u32 {
    w.count_ones()
}

#[unsafe(no_mangle)]
pub extern "C" fn __sw_hweight32(w: u32) -> u32 {
    w.count_ones()
}

ksym!(__sw_hweight64);
ksym!(__sw_hweight32);
