// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! `drm_client_*` shim — DRM's in-kernel client API (used by
//! fbcon / drm_fbdev_*).
//!
//! drm_ttm_helper references these for registering / releasing
//! the dumb-buffer client; no caller fires at load.

use core::ffi::c_void;

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn drm_client_buffer_create_dumb(
    _client: *mut c_void,
    _width: u32,
    _height: u32,
    _format: u32,
) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_client_buffer_delete(_buffer: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn drm_client_buffer_vmap_local(
    _buffer: *mut c_void,
    _map: *mut c_void,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_client_buffer_vunmap_local(_buffer: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn drm_client_release(_client: *mut c_void) {}

ksym!(drm_client_buffer_create_dumb);
ksym!(drm_client_buffer_delete);
ksym!(drm_client_buffer_vmap_local);
ksym!(drm_client_buffer_vunmap_local);
ksym!(drm_client_release);

// K16: drm_dma_helper uses the non-`_local` variants.

#[unsafe(no_mangle)]
pub extern "C" fn drm_client_buffer_vmap(
    _buffer: *mut c_void,
    _map: *mut c_void,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_client_buffer_vunmap(_buffer: *mut c_void) {}

ksym!(drm_client_buffer_vmap);
ksym!(drm_client_buffer_vunmap);
