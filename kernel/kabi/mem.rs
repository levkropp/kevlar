// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! `memcpy` / `memcpy_toio` kABI exports.
//!
//! `memcpy` is implemented in `platform/mem.rs` as a `no_mangle`
//! extern "C" function — the symbol exists in our linked binary.
//! We just need a Rust-visible reference so `ksym!` can take its
//! address.  The `extern "C"` block reaches it without depending
//! on the platform module's Rust-side visibility.
//!
//! `memcpy_toio` handles the "copy to memory-mapped IO" case — on
//! aarch64 it's the same as memcpy at our level (IO accesses go
//! through plain ldr/str; no special barrier required for the
//! K15 stub corpus).

unsafe extern "C" {
    fn memcpy(dest: *mut u8, src: *const u8, n: usize) -> *mut u8;
}

crate::ksym!(memcpy);
crate::ksym_named!("memcpy_toio", memcpy);
