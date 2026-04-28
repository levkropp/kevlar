// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux tracepoint stubs — `__tracepoint_*` and `log_*_mmio`
//! glue used by the rwmmio (read/write MMIO) tracepoint family.
//!
//! Real Linux uses a `static_key` jump table that fires the
//! `log_*_mmio` callbacks only when tracing is enabled.  Modules
//! that probe a memory-mapped region reference these symbols
//! unconditionally because the macro expansion produces a call
//! into the static-key prologue.
//!
//! K17: every entry is a no-op; tracepoints are disabled at all
//! call sites.  When K30+ wires real ktrace, we revisit.

use core::ffi::c_void;

use crate::ksym;

// `__tracepoint_<name>` is a static struct in real Linux; we
// expose a 32-byte zero buffer per name to satisfy the symbol.
#[unsafe(no_mangle)]
pub static __tracepoint_rwmmio_post_read: [u8; 32] = [0; 32];
#[unsafe(no_mangle)]
pub static __tracepoint_rwmmio_post_write: [u8; 32] = [0; 32];
#[unsafe(no_mangle)]
pub static __tracepoint_rwmmio_read: [u8; 32] = [0; 32];
#[unsafe(no_mangle)]
pub static __tracepoint_rwmmio_write: [u8; 32] = [0; 32];

crate::ksym_static!(__tracepoint_rwmmio_post_read);
crate::ksym_static!(__tracepoint_rwmmio_post_write);
crate::ksym_static!(__tracepoint_rwmmio_read);
crate::ksym_static!(__tracepoint_rwmmio_write);

#[unsafe(no_mangle)]
pub extern "C" fn log_post_read_mmio(
    _val: u64,
    _width: u8,
    _addr: *const c_void,
    _caller: *const c_void,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn log_post_write_mmio(
    _val: u64,
    _width: u8,
    _addr: *const c_void,
    _caller: *const c_void,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn log_read_mmio(
    _width: u8,
    _addr: *const c_void,
    _caller: *const c_void,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn log_write_mmio(
    _val: u64,
    _width: u8,
    _addr: *const c_void,
    _caller: *const c_void,
) {
}

ksym!(log_post_read_mmio);
ksym!(log_post_write_mmio);
ksym!(log_read_mmio);
ksym!(log_write_mmio);
