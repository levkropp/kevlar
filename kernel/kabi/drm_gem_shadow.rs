// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! DRM GEM shadow-plane / framebuffer / shmem helpers.
//!
//! Shadow planes carry a CPU-mapped copy of the framebuffer for
//! drivers (like cirrus-qemu) that need to read GPU-side memory
//! before scanout.  K17 stubs: link-only.

use core::ffi::c_void;

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn drm_gem_begin_shadow_fb_access(
    _plane: *mut c_void,
    _state: *mut c_void,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_gem_end_shadow_fb_access(
    _plane: *mut c_void,
    _state: *mut c_void,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_gem_destroy_shadow_plane_state(
    _plane: *mut c_void,
    _state: *mut c_void,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_gem_duplicate_shadow_plane_state(
    _plane: *mut c_void,
) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_gem_reset_shadow_plane(_plane: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn drm_gem_fb_create_with_dirty(
    _dev: *mut c_void,
    _filp: *mut c_void,
    _cmd: *const c_void,
) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_gem_get_unmapped_area(
    _filp: *mut c_void,
    _addr: usize,
    _len: usize,
    _pgoff: usize,
    _flags: u32,
) -> usize {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_gem_mmap(
    _filp: *mut c_void,
    _vma: *mut c_void,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_gem_shmem_dumb_create(
    _filp: *mut c_void,
    _dev: *mut c_void,
    _args: *mut c_void,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_gem_shmem_prime_import_no_map(
    _dev: *mut c_void,
    _dmabuf: *mut c_void,
) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_fbdev_shmem_driver_fbdev_probe(
    _fb_helper: *mut c_void,
    _sizes: *mut c_void,
) -> i32 {
    0
}

ksym!(drm_gem_begin_shadow_fb_access);
ksym!(drm_gem_end_shadow_fb_access);
ksym!(drm_gem_destroy_shadow_plane_state);
ksym!(drm_gem_duplicate_shadow_plane_state);
ksym!(drm_gem_reset_shadow_plane);
ksym!(drm_gem_fb_create_with_dirty);
ksym!(drm_gem_get_unmapped_area);
ksym!(drm_gem_mmap);
ksym!(drm_gem_shmem_dumb_create);
ksym!(drm_gem_shmem_prime_import_no_map);
ksym!(drm_fbdev_shmem_driver_fbdev_probe);
