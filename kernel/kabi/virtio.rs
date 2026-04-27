// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux virtio-bus core stubs (K12).
//!
//! `__register_virtio_driver` is the only function called from
//! virtio_input.ko's `init_module`.  The rest are vring helpers
//! used by probe/remove paths that fire only when the bus matches
//! a driver to a device — which our stub registration doesn't do.

use core::ffi::c_void;

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn __register_virtio_driver(
    _drv: *mut c_void,
    _owner: *const c_void,
) -> i32 {
    log::info!("kabi: __register_virtio_driver (stub)");
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn unregister_virtio_driver(_drv: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn virtio_reset_device(_dev: *mut c_void) {}

// virtqueue_*: ring-buffer helpers.  Probe-path only.

#[unsafe(no_mangle)]
pub extern "C" fn virtqueue_add_inbuf_cache_clean(
    _vq: *mut c_void,
    _sg: *mut c_void,
    _num: u32,
    _data: *mut c_void,
    _ctx: *mut c_void,
    _gfp: u32,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn virtqueue_add_outbuf(
    _vq: *mut c_void,
    _sg: *mut c_void,
    _num: u32,
    _data: *mut c_void,
    _gfp: u32,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn virtqueue_detach_unused_buf(_vq: *mut c_void) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn virtqueue_get_buf(
    _vq: *mut c_void,
    _len: *mut u32,
) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn virtqueue_get_vring_size(_vq: *const c_void) -> u32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn virtqueue_kick(_vq: *mut c_void) -> bool {
    true
}

ksym!(__register_virtio_driver);
ksym!(unregister_virtio_driver);
ksym!(virtio_reset_device);
ksym!(virtqueue_add_inbuf_cache_clean);
ksym!(virtqueue_add_outbuf);
ksym!(virtqueue_detach_unused_buf);
ksym!(virtqueue_get_buf);
ksym!(virtqueue_get_vring_size);
ksym!(virtqueue_kick);
