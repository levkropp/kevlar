// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Software framebuffer raster ops (`sys_copyarea`, `sys_fillrect`,
//! `sys_imageblit`).
//!
//! These are what fbcon uses to draw text + scroll a framebuffer
//! when the display has no hardware acceleration.  Linux's real
//! implementations memcpy / memset over the framebuffer at byte/
//! pixel granularity.
//!
//! K15: stubs only — drm_ttm_helper references them but doesn't
//! fire them at load.  K20+ when an actual fbcon surface tries to
//! draw, we replace with real raster routines.

use core::ffi::c_void;

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn sys_copyarea(_info: *mut c_void, _area: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn sys_fillrect(_info: *mut c_void, _rect: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn sys_imageblit(_info: *mut c_void, _image: *mut c_void) {}

ksym!(sys_copyarea);
ksym!(sys_fillrect);
ksym!(sys_imageblit);
