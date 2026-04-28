// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Framebuffer (`fb_*`) shim — Linux's legacy fbdev surface.
//!
//! `fb_deferred_io_*` manages the deferred-IO machinery that fbcon
//! uses to coalesce dirty regions; `fb_sys_read/write` are the
//! sysfs-style read/write helpers fbdev exposes.  drm_ttm_helper
//! references all five; nothing fires at load.

use core::ffi::c_void;

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn fb_deferred_io_init(_info: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn fb_deferred_io_cleanup(_info: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn fb_deferred_io_mmap(
    _info: *mut c_void,
    _vma: *mut c_void,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn fb_sys_read(
    _info: *mut c_void,
    _buf: *mut c_void,
    _count: usize,
    _ppos: *mut c_void,
) -> isize {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn fb_sys_write(
    _info: *mut c_void,
    _buf: *const c_void,
    _count: usize,
    _ppos: *mut c_void,
) -> isize {
    0
}

ksym!(fb_deferred_io_init);
ksym!(fb_deferred_io_cleanup);
ksym!(fb_deferred_io_mmap);
ksym!(fb_sys_read);
ksym!(fb_sys_write);
