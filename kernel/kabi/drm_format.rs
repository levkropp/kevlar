// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! DRM pixel-format helpers (`drm_format_info_*`,
//! `drm_driver_legacy_fb_format`).
//!
//! drm_ttm_helper's fbdev path queries pixel-format metadata to
//! decide bytes-per-pixel and legacy fbdev format codes.  K15
//! returns zero — no caller fires at load.

use core::ffi::c_void;

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn drm_format_info_bpp(
    _info: *const c_void,
    _plane: i32,
) -> u32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_driver_legacy_fb_format(
    _dev: *const c_void,
    _bpp: u32,
    _depth: u32,
) -> u32 {
    0
}

ksym!(drm_format_info_bpp);
ksym!(drm_driver_legacy_fb_format);
