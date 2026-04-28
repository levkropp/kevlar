// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux `refcount_t` saturation handler.
//!
//! When a refcount overflows or goes negative, Linux's `refcount.c`
//! calls `refcount_warn_saturate` to log + saturate the count
//! (rather than wrap and risk use-after-free).  K14: log + ignore.

use core::ffi::c_void;

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn refcount_warn_saturate(
    _r: *mut c_void,
    _t: u32,
) {
    log::warn!("kabi: refcount_warn_saturate (stub)");
}

ksym!(refcount_warn_saturate);
