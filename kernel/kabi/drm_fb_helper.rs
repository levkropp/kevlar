// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! `drm_fb_helper_*` shim — DRM's framebuffer-emulation helpers.
//!
//! When a DRM driver wants to expose `/dev/fb0` (Linux's legacy
//! framebuffer / fbcon interface) on top of a GPU buffer, it
//! registers callbacks against this set.  drm_ttm_helper.ko
//! references all 11 of them; nothing fires at load time because
//! the module has no `init_module` and is purely library code
//! consumed by other DRM drivers.
//!
//! K15: every entry is a no-op / success-returning stub.  K20+
//! when an actual DRM driver registers a real fbdev surface, we
//! revisit.

use core::ffi::c_void;

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn drm_fb_helper_blank(_blank: i32, _info: *mut c_void) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_fb_helper_check_var(
    _var: *mut c_void,
    _info: *mut c_void,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_fb_helper_damage_area(
    _info: *mut c_void,
    _x: u32,
    _y: u32,
    _w: u32,
    _h: u32,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_fb_helper_damage_range(
    _info: *mut c_void,
    _off: usize,
    _len: usize,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_fb_helper_deferred_io(
    _info: *mut c_void,
    _pagereflist: *mut c_void,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_fb_helper_fill_info(
    _info: *mut c_void,
    _fb_helper: *mut c_void,
    _sizes: *mut c_void,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_fb_helper_fini(_fb_helper: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn drm_fb_helper_ioctl(
    _info: *mut c_void,
    _cmd: u32,
    _arg: usize,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_fb_helper_pan_display(
    _var: *mut c_void,
    _info: *mut c_void,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_fb_helper_set_par(_info: *mut c_void) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_fb_helper_setcmap(
    _cmap: *mut c_void,
    _info: *mut c_void,
) -> i32 {
    0
}

ksym!(drm_fb_helper_blank);
ksym!(drm_fb_helper_check_var);
ksym!(drm_fb_helper_damage_area);
ksym!(drm_fb_helper_damage_range);
ksym!(drm_fb_helper_deferred_io);
ksym!(drm_fb_helper_fill_info);
ksym!(drm_fb_helper_fini);
ksym!(drm_fb_helper_ioctl);
ksym!(drm_fb_helper_pan_display);
ksym!(drm_fb_helper_set_par);
ksym!(drm_fb_helper_setcmap);
