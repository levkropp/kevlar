// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! DRM EDID (Extended Display Identification Data) shim.
//!
//! EDID is the byte sequence a monitor returns over DDC/HDMI to
//! identify itself (resolutions supported, manufacturer, serial,
//! etc.).  bochs.ko reads a synthetic EDID from the QEMU
//! bochs-display PCI BAR.  K18: stub all 5 — no caller fires at
//! K18 (probe doesn't run).
//!
//! K22+ when real display modes need to be enumerated and offered
//! to userspace, we revisit with a real parser.

use core::ffi::c_void;

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn drm_edid_connector_add_modes(
    _connector: *mut c_void,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_edid_connector_update(
    _connector: *mut c_void,
    _drm_edid: *const c_void,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_edid_free(_drm_edid: *const c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn drm_edid_header_is_valid(_buf: *const c_void) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_edid_read_custom(
    _connector: *mut c_void,
    _read_block: *mut c_void,
    _context: *mut c_void,
) -> *const c_void {
    core::ptr::null()
}

ksym!(drm_edid_connector_add_modes);
ksym!(drm_edid_connector_update);
ksym!(drm_edid_free);
ksym!(drm_edid_header_is_valid);
ksym!(drm_edid_read_custom);
