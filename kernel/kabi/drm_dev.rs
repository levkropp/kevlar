// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! DRM device lifecycle / fops shim.
//!
//! `__devm_drm_dev_alloc`, `drm_dev_register`, `drm_dev_unplug`,
//! `drm_dev_enter`/`_exit`, and the standard fops entries
//! (`drm_open`/`_release`/`_read`/`_poll`/`_ioctl`/`_compat_ioctl`)
//! make up the surface a real DRM driver depends on at probe /
//! release / runtime.  K17 has no probe firing, so all are link-
//! only no-ops.
//!
//! `drmm_mode_config_init` is a managed-init helper — Linux's
//! real implementation registers a cleanup callback against the
//! drm_device's drm_managed allocator.  K17 stub is no-op.

use core::ffi::c_void;

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn __devm_drm_dev_alloc(
    _parent: *mut c_void,
    _driver: *const c_void,
    _size: usize,
    _offset: usize,
) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_dev_register(_dev: *mut c_void, _flags: u64) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_dev_unplug(_dev: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn drm_dev_enter(_dev: *mut c_void, _idx: *mut i32) -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_dev_exit(_idx: i32) {}

#[unsafe(no_mangle)]
pub extern "C" fn drmm_mode_config_init(_dev: *mut c_void) -> i32 {
    0
}

// ── DRM file_operations callbacks ─────────────────────────────
// All take struct file * (and friends).  Probe doesn't fire,
// userspace doesn't open /dev/dri/cardN, none get called.

#[unsafe(no_mangle)]
pub extern "C" fn drm_open(_inode: *mut c_void, _filp: *mut c_void) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_release(_inode: *mut c_void, _filp: *mut c_void) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_read(
    _filp: *mut c_void,
    _buf: *mut c_void,
    _count: usize,
    _ppos: *mut c_void,
) -> isize {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_poll(_filp: *mut c_void, _wait: *mut c_void) -> u32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_ioctl(
    _filp: *mut c_void,
    _cmd: u32,
    _arg: usize,
) -> isize {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_compat_ioctl(
    _filp: *mut c_void,
    _cmd: u32,
    _arg: usize,
) -> isize {
    0
}

ksym!(__devm_drm_dev_alloc);
ksym!(drm_dev_register);
ksym!(drm_dev_unplug);
ksym!(drm_dev_enter);
ksym!(drm_dev_exit);
ksym!(drmm_mode_config_init);
ksym!(drm_open);
ksym!(drm_release);
ksym!(drm_read);
ksym!(drm_poll);
ksym!(drm_ioctl);
ksym!(drm_compat_ioctl);

/// `drm_dev_put(dev)` — drop a refcount on a drm_device.  K18:
/// no-op (no refcounts tracked yet).
#[unsafe(no_mangle)]
pub extern "C" fn drm_dev_put(_dev: *mut c_void) {}

ksym!(drm_dev_put);
