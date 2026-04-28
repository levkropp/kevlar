// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! DRM PRIME (cross-driver buffer sharing) shim + dumb-mode helper.
//!
//! drm_dma_helper references three of these — none fire at K16
//! load (drm_dma_helper has no `init_module`).

use core::ffi::c_void;

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn drm_mode_size_dumb(
    _dev: *mut c_void,
    _args: *mut c_void,
    _min_pitch: u32,
    _pitch_align: u32,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_prime_gem_destroy(
    _obj: *mut c_void,
    _sg: *mut c_void,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_prime_get_contiguous_size(_sgt: *mut c_void) -> usize {
    0
}

ksym!(drm_mode_size_dumb);
ksym!(drm_prime_gem_destroy);
ksym!(drm_prime_get_contiguous_size);
