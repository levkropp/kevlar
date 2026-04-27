// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux UBSan handler stubs.
//!
//! `__ubsan_handle_*` are compiler-injected callbacks that fire
//! when a runtime undefined-behavior check trips.  They normally
//! point at Linux's `lib/ubsan.c` reporting infrastructure.
//!
//! K12: log + continue.  Real corruption gets noticed in the
//! log; we don't panic because UBSan's checks include relatively
//! benign cases (out-of-bounds reads of `__attribute__((aligned))`
//! padding) that Linux still functions through.

use core::ffi::c_void;

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn __ubsan_handle_load_invalid_value(
    _data: *mut c_void,
    _value: *mut c_void,
) {
    log::warn!("kabi: UBSan load_invalid_value");
}

#[unsafe(no_mangle)]
pub extern "C" fn __ubsan_handle_out_of_bounds(
    _data: *mut c_void,
    _index: *mut c_void,
) {
    log::warn!("kabi: UBSan out_of_bounds");
}

#[unsafe(no_mangle)]
pub extern "C" fn __ubsan_handle_shift_out_of_bounds(
    _data: *mut c_void,
    _lhs: *mut c_void,
    _rhs: *mut c_void,
) {
    log::warn!("kabi: UBSan shift_out_of_bounds");
}

ksym!(__ubsan_handle_load_invalid_value);
ksym!(__ubsan_handle_out_of_bounds);
ksym!(__ubsan_handle_shift_out_of_bounds);
