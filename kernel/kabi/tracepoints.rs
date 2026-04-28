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

// ── BPF / perf trace trampolines ────────────────────────────────
// Linux's `TRACE_EVENT()` macro emits an inline call to
// `bpf_trace_runN` / `perf_trace_run_bpf_submit` /
// `perf_trace_buf_alloc` for each tracepoint, plus the
// `trace_event_*` helpers used by the format-string event-class
// machinery.  No-op stubs since we don't run tracing on loaded
// modules.

#[unsafe(no_mangle)]
pub extern "C" fn bpf_trace_run1(_prog: *mut c_void,
                                 _ctx: *mut c_void) -> u32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn bpf_trace_run3(_prog: *mut c_void, _a1: u64,
                                 _a2: u64, _a3: u64) -> u32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn bpf_trace_run4(_prog: *mut c_void, _a1: u64,
                                 _a2: u64, _a3: u64, _a4: u64) -> u32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn perf_trace_buf_alloc(_size: u32, _regs: *mut c_void,
                                       _rctx: *mut core::ffi::c_int)
                                       -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn perf_trace_run_bpf_submit(_raw_data: *mut c_void,
                                            _size: u32, _rctx: i32,
                                            _event_class: *mut c_void,
                                            _event: *mut c_void,
                                            _regs: *mut c_void,
                                            _head: *mut c_void,
                                            _task: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn trace_event_buffer_reserve(_buffer: *mut c_void,
                                             _trace_file: *mut c_void,
                                             _len: u32) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn trace_event_buffer_commit(_buffer: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn trace_event_raw_init(_call: *mut c_void) -> i32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn trace_event_reg(_call: *mut c_void, _type: u32,
                                  _data: *mut c_void) -> i32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn trace_event_printf(_event: *mut c_void,
                                     _fmt: *const u8) {}

#[unsafe(no_mangle)]
pub extern "C" fn trace_handle_return(_seq: *mut c_void) -> i32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn trace_print_flags_seq(_p: *mut c_void, _delim: *const u8,
                                        _flag_array: *mut c_void,
                                        _flag_array_size: u32) -> *const u8 {
    b"\0".as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn trace_print_symbols_seq(_p: *mut c_void, _val: u64,
                                          _symbol_array: *mut c_void)
                                          -> *const u8 {
    b"\0".as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn trace_raw_output_prep(_iter: *mut c_void,
                                        _trace_event: *mut c_void)
                                        -> u32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn __trace_trigger_soft_disabled(_file: *mut c_void) -> bool {
    true // "trigger soft-disabled" — caller skips slow path
}

ksym!(bpf_trace_run1);
ksym!(bpf_trace_run3);
ksym!(bpf_trace_run4);
ksym!(perf_trace_buf_alloc);
ksym!(perf_trace_run_bpf_submit);
ksym!(trace_event_buffer_reserve);
ksym!(trace_event_buffer_commit);
ksym!(trace_event_raw_init);
ksym!(trace_event_reg);
ksym!(trace_event_printf);
ksym!(trace_handle_return);
ksym!(trace_print_flags_seq);
ksym!(trace_print_symbols_seq);
ksym!(trace_raw_output_prep);
ksym!(__trace_trigger_soft_disabled);

// ── tracepoint_srcu — sleeping-RCU read-side lock holder used
// by tracepoints.  We hand back a fake non-null pointer; the
// caller's "if(srcu)" check passes, no real RCU work happens.
#[unsafe(no_mangle)]
pub static tracepoint_srcu: [u8; 64] = [0; 64];
crate::ksym_static!(tracepoint_srcu);
