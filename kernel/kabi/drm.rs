// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! DRM (Direct Rendering Manager) shim — start of the DRM stack.
//!
//! K13 entry: `drm_printf`, the variadic helper DRM modules use to
//! emit messages through `struct drm_printer`.  We ignore the printer
//! object and route the format string through the K6 printk
//! formatter, mirroring how `printk`/`_dev_err` operate.

use core::ffi::{c_char, c_void};

use crate::kabi::printk_fmt::{format_into, Sink};
use crate::ksym;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn drm_printf(
    _printer: *mut c_void,
    fmt: *const c_char,
    mut args: ...
) {
    if fmt.is_null() {
        return;
    }
    let mut buf = [0u8; 512];
    let n = {
        let mut sink = Sink::new(&mut buf);
        unsafe { format_into(&mut sink, fmt, &mut args) };
        sink.pos()
    };
    if let Ok(s) = core::str::from_utf8(&buf[..n]) {
        log::info!("[mod-drm] {}", s.trim_end_matches('\n'));
    }
}

ksym!(drm_printf);

/// `drm_print_bits(p, val, bits, nbits)` — print a bitfield via
/// the printer.  K15: log + ignore.
#[unsafe(no_mangle)]
pub extern "C" fn drm_print_bits(
    _printer: *mut c_void,
    _value: u64,
    _bits: *const c_void,
    _nbits: u32,
) {
}

ksym!(drm_print_bits);

/// `__drm_dev_dbg(category, dev, fmt, ...)` — DRM's variadic
/// per-category debug print.  Linux gates it on a sysfs flag;
/// we just route through the K6 formatter.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __drm_dev_dbg(
    _category: u32,
    _dev: *const c_void,
    fmt: *const c_char,
    mut args: ...
) {
    if fmt.is_null() {
        return;
    }
    let mut buf = [0u8; 512];
    let n = {
        let mut sink = Sink::new(&mut buf);
        unsafe { format_into(&mut sink, fmt, &mut args) };
        sink.pos()
    };
    if let Ok(s) = core::str::from_utf8(&buf[..n]) {
        log::debug!("[mod-drm-dbg] {}", s.trim_end_matches('\n'));
    }
}

ksym!(__drm_dev_dbg);

/// `drm_gem_object_lookup(filp, handle)` — find a gem object by
/// handle.  K15: no objects exist; return null.
#[unsafe(no_mangle)]
pub extern "C" fn drm_gem_object_lookup(
    _filp: *mut c_void,
    _handle: u32,
) -> *mut c_void {
    core::ptr::null_mut()
}

ksym!(drm_gem_object_lookup);
