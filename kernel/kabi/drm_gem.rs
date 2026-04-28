// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! DRM GEM (Graphics Execution Manager) shim.
//!
//! `drm_gem_object_free` is a kref release callback that DRM modules
//! register on their gem objects.  drm_exec only references the
//! symbol; nothing invokes it at K14 load.  K15+ when a real DRM
//! driver actually creates and releases gem objects, this becomes
//! load-bearing.

use core::ffi::c_void;

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn drm_gem_object_free(_kref: *mut c_void) {}

ksym!(drm_gem_object_free);

// ── K16 GEM API extensions for drm_dma_helper.ko ──────────────
// All link-only at K16; no probe runs.

#[unsafe(no_mangle)]
pub extern "C" fn drm_gem_create_mmap_offset(_obj: *mut c_void) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_gem_fb_get_obj(
    _fb: *mut c_void,
    _plane: u32,
) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_gem_handle_create(
    _filp: *mut c_void,
    _obj: *mut c_void,
    _handle: *mut u32,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_gem_object_init(
    _dev: *mut c_void,
    _obj: *mut c_void,
    _size: usize,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_gem_object_release(_obj: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn drm_gem_prime_mmap(
    _obj: *mut c_void,
    _vma: *mut c_void,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_gem_private_object_init(
    _dev: *mut c_void,
    _obj: *mut c_void,
    _size: usize,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_gem_vm_open(_vma: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn drm_gem_vm_close(_vma: *mut c_void) {}

ksym!(drm_gem_create_mmap_offset);
ksym!(drm_gem_fb_get_obj);
ksym!(drm_gem_handle_create);
ksym!(drm_gem_object_init);
ksym!(drm_gem_object_release);
ksym!(drm_gem_prime_mmap);
ksym!(drm_gem_private_object_init);
ksym!(drm_gem_vm_open);
ksym!(drm_gem_vm_close);
