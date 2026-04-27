// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! `printk` — the one kernel symbol K1 exports to loaded `.ko` modules.
//!
//! Real Linux's `printk` is variadic (`int printk(const char *fmt,
//! ...)`).  Variadic ABI isn't stable in Rust, so K1's stub takes
//! only `(*const c_char)` — it prints the format string verbatim,
//! ignoring `%`-tokens.  Sufficient for the hello-world demo.
//!
//! K2 will swap this for a Linux-shaped variadic via a small inline
//! printf-class formatter.

use crate::ksym;

/// Walk `fmt` to NUL (capped at 4KB), decode as UTF-8, and emit via
/// the kernel's existing `info!()` infra.  Tagged `[mod]` so it's
/// distinguishable from native kernel logs.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn printk(fmt: *const core::ffi::c_char) {
    if fmt.is_null() {
        return;
    }
    #[allow(unsafe_code)]
    unsafe {
        let mut n = 0usize;
        while n < 4096 && *fmt.add(n) != 0 {
            n += 1;
        }
        let bytes = core::slice::from_raw_parts(fmt as *const u8, n);
        if let Ok(s) = core::str::from_utf8(bytes) {
            log::info!("[mod] {}", s.trim_end_matches('\n'));
        }
    }
}

ksym!(printk);
