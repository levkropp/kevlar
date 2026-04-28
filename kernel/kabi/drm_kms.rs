// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! DRM KMS (Kernel Mode Setting) shim — atomic helpers + CRTC /
//! encoder / connector / plane / vblank object lifecycle.
//!
//! The 31 functions here are all link-only at K17 — cirrus-qemu's
//! init_module just calls `pci_register_driver()` and returns.
//! Probe (where these would fire) is K20+.

use core::ffi::c_void;

use crate::ksym;

// ── Atomic-modeset helpers ────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn drm_atomic_helper_check(
    _dev: *mut c_void,
    _state: *mut c_void,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_atomic_helper_commit(
    _dev: *mut c_void,
    _state: *mut c_void,
    _nonblock: bool,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_atomic_helper_check_crtc_primary_plane(
    _crtc_state: *mut c_void,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_atomic_helper_check_plane_state(
    _plane_state: *mut c_void,
    _crtc_state: *const c_void,
    _min_scale: i32,
    _max_scale: i32,
    _can_position: bool,
    _can_update_disabled: bool,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_atomic_helper_connector_destroy_state(
    _connector: *mut c_void,
    _state: *mut c_void,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_atomic_helper_connector_duplicate_state(
    _connector: *mut c_void,
) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_atomic_helper_connector_reset(_connector: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn drm_atomic_helper_crtc_destroy_state(
    _crtc: *mut c_void,
    _state: *mut c_void,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_atomic_helper_crtc_duplicate_state(
    _crtc: *mut c_void,
) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_atomic_helper_crtc_reset(_crtc: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn drm_atomic_helper_disable_plane(
    _plane: *mut c_void,
    _ctx: *mut c_void,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_atomic_helper_page_flip(
    _crtc: *mut c_void,
    _fb: *mut c_void,
    _event: *mut c_void,
    _flags: u32,
    _ctx: *mut c_void,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_atomic_helper_set_config(
    _set: *mut c_void,
    _ctx: *mut c_void,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_atomic_helper_shutdown(_dev: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn drm_atomic_helper_update_plane(
    _plane: *mut c_void,
    _crtc: *mut c_void,
    _fb: *mut c_void,
    _crtc_x: i32,
    _crtc_y: i32,
    _crtc_w: u32,
    _crtc_h: u32,
    _src_x: u32,
    _src_y: u32,
    _src_w: u32,
    _src_h: u32,
    _ctx: *mut c_void,
) -> i32 {
    0
}

ksym!(drm_atomic_helper_check);
ksym!(drm_atomic_helper_commit);
ksym!(drm_atomic_helper_check_crtc_primary_plane);
ksym!(drm_atomic_helper_check_plane_state);
ksym!(drm_atomic_helper_connector_destroy_state);
ksym!(drm_atomic_helper_connector_duplicate_state);
ksym!(drm_atomic_helper_connector_reset);
ksym!(drm_atomic_helper_crtc_destroy_state);
ksym!(drm_atomic_helper_crtc_duplicate_state);
ksym!(drm_atomic_helper_crtc_reset);
ksym!(drm_atomic_helper_disable_plane);
ksym!(drm_atomic_helper_page_flip);
ksym!(drm_atomic_helper_set_config);
ksym!(drm_atomic_helper_shutdown);
ksym!(drm_atomic_helper_update_plane);

// ── CRTC / encoder / connector / plane / vblank ────────────────

#[unsafe(no_mangle)]
pub extern "C" fn drm_crtc_cleanup(_crtc: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn drm_crtc_init_with_planes(
    _dev: *mut c_void,
    _crtc: *mut c_void,
    _primary: *mut c_void,
    _cursor: *mut c_void,
    _funcs: *const c_void,
    _name: *const c_void,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_crtc_vblank_atomic_disable(_crtc: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn drm_crtc_vblank_atomic_flush(_crtc: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn drm_crtc_vblank_helper_disable_vblank_timer(_crtc: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn drm_crtc_vblank_helper_enable_vblank_timer(_crtc: *mut c_void) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_crtc_vblank_helper_get_vblank_timestamp_from_timer(
    _crtc: *mut c_void,
    _max_error: *mut i32,
    _ts: *mut c_void,
    _flags: u32,
) -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_crtc_vblank_on(_crtc: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn drm_encoder_cleanup(_encoder: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn drm_encoder_init(
    _dev: *mut c_void,
    _encoder: *mut c_void,
    _funcs: *const c_void,
    _encoder_type: i32,
    _name: *const c_void,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_universal_plane_init(
    _dev: *mut c_void,
    _plane: *mut c_void,
    _crtcs: u32,
    _funcs: *const c_void,
    _formats: *const u32,
    _format_count: u32,
    _format_modifiers: *const u64,
    _plane_type: u32,
    _name: *const c_void,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_plane_cleanup(_plane: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn drm_plane_enable_fb_damage_clips(_plane: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn drm_vblank_init(_dev: *mut c_void, _num: u32) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_connector_attach_encoder(
    _connector: *mut c_void,
    _encoder: *mut c_void,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_connector_cleanup(_connector: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn drm_connector_init(
    _dev: *mut c_void,
    _connector: *mut c_void,
    _funcs: *const c_void,
    _connector_type: i32,
) -> i32 {
    0
}

ksym!(drm_crtc_cleanup);
ksym!(drm_crtc_init_with_planes);
ksym!(drm_crtc_vblank_atomic_disable);
ksym!(drm_crtc_vblank_atomic_flush);
ksym!(drm_crtc_vblank_helper_disable_vblank_timer);
ksym!(drm_crtc_vblank_helper_enable_vblank_timer);
ksym!(drm_crtc_vblank_helper_get_vblank_timestamp_from_timer);
ksym!(drm_crtc_vblank_on);
ksym!(drm_encoder_cleanup);
ksym!(drm_encoder_init);
ksym!(drm_universal_plane_init);
ksym!(drm_plane_cleanup);
ksym!(drm_plane_enable_fb_damage_clips);
ksym!(drm_vblank_init);
ksym!(drm_connector_attach_encoder);
ksym!(drm_connector_cleanup);
ksym!(drm_connector_init);
