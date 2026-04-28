// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! DRM atomic-modeset damage iterator shim.
//!
//! When a real KMS update commits, `drm_atomic_helper_damage_iter_*`
//! walks the dirty-rect list to drive partial display refresh.  K16
//! has no real commit firing; both calls are no-ops.

use core::ffi::c_void;

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn drm_atomic_helper_damage_iter_init(
    _iter: *mut c_void,
    _old_state: *const c_void,
    _state: *const c_void,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_atomic_helper_damage_iter_next(
    _iter: *mut c_void,
    _rect: *mut c_void,
) -> bool {
    false
}

ksym!(drm_atomic_helper_damage_iter_init);
ksym!(drm_atomic_helper_damage_iter_next);
