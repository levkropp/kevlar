// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! TTM (Translation Table Manager) buffer-object shim.
//!
//! TTM is DRM's GPU-memory abstraction layer.  drm_ttm_helper
//! references three TTM bo (buffer object) operations; no caller
//! fires at K15 load.

use core::ffi::c_void;

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn ttm_bo_mmap_obj(
    _vma: *mut c_void,
    _bo: *mut c_void,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn ttm_bo_vmap(_bo: *mut c_void, _map: *mut c_void) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn ttm_bo_vunmap(_bo: *mut c_void, _map: *mut c_void) {}

ksym!(ttm_bo_mmap_obj);
ksym!(ttm_bo_vmap);
ksym!(ttm_bo_vunmap);
