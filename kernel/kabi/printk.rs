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
