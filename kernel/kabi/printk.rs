// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux-shaped variadic `printk`.  K6: real format-string parsing
//! supporting the subset every standard kernel module uses
//! (%d/%i/%u/%x/%X/%p/%s/%c/%% with width + zero-pad +
//! length-modifier flags).
//!
//! Implementation lives in `printk_fmt.rs`; this module is the
//! ksym!-exported entry point.

use core::ffi::c_char;

use crate::kabi::printk_fmt::{format_into, Sink};
use crate::ksym;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn printk(fmt: *const c_char, mut args: ...) -> i32 {
    if fmt.is_null() {
        return 0;
    }
    let mut buf = [0u8; 1024];
    let n = {
        let mut sink = Sink::new(&mut buf);
        unsafe { format_into(&mut sink, fmt, &mut args) };
        sink.pos()
    };
    if let Ok(s) = core::str::from_utf8(&buf[..n]) {
        log::info!("[mod] {}", s.trim_end_matches('\n'));
    }
    n as i32
}

ksym!(printk);

// Linux 7.0 made printk() a macro that wraps `_printk()` for the
// printk-index machinery.  Modules compiled against
// `<linux/printk.h>` reference `_printk` directly; alias to our
// printk so resolution succeeds.
crate::ksym_named!("_printk", printk);

/// Linux's `snprintf(buf, size, fmt, ...)` formats into a
/// caller-provided buffer.  Same format-spec parser as printk;
/// just sinks the output to the user buffer instead of log.
/// Returns the number of characters that would have been written
/// (matching glibc / Linux semantics).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snprintf(
    buf: *mut c_char,
    size: usize,
    fmt: *const c_char,
    mut args: ...
) -> i32 {
    if buf.is_null() || size == 0 {
        return 0;
    }
    let dst = unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, size) };
    // Reserve the last byte for NUL.
    let cap = size - 1;
    let written = {
        let mut sink = Sink::new(&mut dst[..cap]);
        unsafe { format_into(&mut sink, fmt, &mut args) };
        sink.pos()
    };
    dst[written] = 0;
    written as i32
}

ksym!(snprintf);

// Linux 7.0 also exports `_dev_err`/`_dev_warn` etc. — variadic
// dev_err with the device pointer ignored at the kABI layer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _dev_err(
    _dev: *const core::ffi::c_void,
    fmt: *const c_char,
    mut args: ...
) -> i32 {
    if fmt.is_null() {
        return 0;
    }
    let mut buf = [0u8; 512];
    let n = {
        let mut sink = Sink::new(&mut buf);
        unsafe { format_into(&mut sink, fmt, &mut args) };
        sink.pos()
    };
    if let Ok(s) = core::str::from_utf8(&buf[..n]) {
        log::warn!("[mod-dev-err] {}", s.trim_end_matches('\n'));
    }
    n as i32
}

ksym!(_dev_err);
