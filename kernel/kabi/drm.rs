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

// ── K17 helpers (cirrus-qemu surface) ─────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn drm_format_info(_format: u32) -> *const c_void {
    core::ptr::null()
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_format_info_min_pitch(
    _info: *const c_void,
    _plane: i32,
    _buffer_width: u32,
) -> u64 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_fb_clip_offset(
    _pitch: u32,
    _format: *const c_void,
    _clip: *const c_void,
) -> usize {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_fb_memcpy(
    _dst: *mut c_void,
    _dst_pitch: *const u32,
    _src: *const c_void,
    _fb: *const c_void,
    _clip: *const c_void,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_set_preferred_mode(
    _connector: *mut c_void,
    _hpref: i32,
    _vpref: i32,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_helper_probe_single_connector_modes(
    _connector: *mut c_void,
    _max_width: u32,
    _max_height: u32,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_add_modes_noedid(
    _connector: *mut c_void,
    _hdisplay: i32,
    _vdisplay: i32,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_client_setup(
    _dev: *mut c_void,
    _format: *const c_void,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_mode_config_reset(_dev: *mut c_void) {}

ksym!(drm_format_info);
ksym!(drm_format_info_min_pitch);
ksym!(drm_fb_clip_offset);
ksym!(drm_fb_memcpy);
ksym!(drm_set_preferred_mode);
ksym!(drm_helper_probe_single_connector_modes);
ksym!(drm_add_modes_noedid);
ksym!(drm_client_setup);
ksym!(drm_mode_config_reset);
