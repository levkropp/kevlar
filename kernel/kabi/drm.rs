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
