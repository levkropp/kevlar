// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux linked-list debug helpers (`__list_*_or_report`).
//!
//! When `CONFIG_LIST_HARDENED` (or `CONFIG_DEBUG_LIST`) is on, every
//! `list_add` / `list_del` is wrapped by a check that calls one of
//! these `_or_report` functions.  If the list is corrupted, the
//! function reports and returns `false`; otherwise it returns `true`.
//!
//! K13 stub: trust the module, return `true` always.

use core::ffi::c_void;

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn __list_add_valid_or_report(
    _new: *mut c_void,
    _prev: *mut c_void,
    _next: *mut c_void,
) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn __list_del_entry_valid_or_report(
    _entry: *mut c_void,
) -> bool {
    true
}

ksym!(__list_add_valid_or_report);
ksym!(__list_del_entry_valid_or_report);
