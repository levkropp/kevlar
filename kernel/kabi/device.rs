// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux `struct device` shim — minimal K3 surface.
//!
//! Module-side layout (matches `testing/include/kevlar_kabi_k3.h`):
//!
//! ```c
//! struct device {
//!     void                       *_kevlar_inner;
//!     struct device              *parent;
//!     const struct bus_type      *bus;
//!     struct device_driver       *driver;
//!     void                       *driver_data;
//!     const char                 *init_name;
//! };
//! ```
//!
//! `_kevlar_inner` points to a heap-allocated `DeviceInner` carrying
//! the refcount + bound flag.  Lazily allocated on
//! `device_initialize` (or any registration entrypoint).

use alloc::boxed::Box;
use core::ffi::{c_char, c_void};
use core::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use crate::kabi::bus::{BusTypeShim, DeviceDriverShim};
use crate::ksym;

#[repr(C)]
pub struct DeviceShim {
    pub inner: *mut DeviceInner,
    pub parent: *mut DeviceShim,
    pub bus: *const BusTypeShim,
    pub driver: *mut DeviceDriverShim,
    pub driver_data: *mut c_void,
    pub init_name: *const c_char,
}

pub struct DeviceInner {
    pub refcount: AtomicI32,
    pub bound: AtomicBool,
}

impl DeviceInner {
    pub fn new() -> Self {
        Self {
            refcount: AtomicI32::new(1),
            bound: AtomicBool::new(false),
        }
    }
}

pub fn ensure_inner(d: *mut DeviceShim) -> *mut DeviceInner {
    if d.is_null() {
        return core::ptr::null_mut();
    }
    let existing = unsafe { (*d).inner };
    if !existing.is_null() {
        return existing;
    }
    let boxed = Box::new(DeviceInner::new());
    let raw = Box::into_raw(boxed);
    unsafe {
        (*d).inner = raw;
    }
    raw
}

#[unsafe(no_mangle)]
pub extern "C" fn device_initialize(d: *mut DeviceShim) {
    let _ = ensure_inner(d);
}

/// Register `d` on its bus.  Triggers a match-walk: every driver
/// registered on the same bus is asked via `bus->match`; the first
/// match has its `probe` invoked.
#[unsafe(no_mangle)]
pub extern "C" fn device_add(d: *mut DeviceShim) -> i32 {
    if d.is_null() {
        return -22;
    }
    ensure_inner(d);
    let bus = unsafe { (*d).bus };
    if bus.is_null() {
        log::warn!("kabi: device_add with null bus");
        return -22;
    }
    crate::kabi::bus::add_device(bus, d);
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn device_register(d: *mut DeviceShim) -> i32 {
    device_initialize(d);
    device_add(d)
}

#[unsafe(no_mangle)]
pub extern "C" fn device_unregister(d: *mut DeviceShim) {
    if d.is_null() {
        return;
    }
    let bus = unsafe { (*d).bus };
    if !bus.is_null() {
        crate::kabi::bus::remove_device(bus, d);
    }
    put_device(d);
}

#[unsafe(no_mangle)]
pub extern "C" fn get_device(d: *mut DeviceShim) -> *mut DeviceShim {
    if d.is_null() {
        return d;
    }
    let inner = ensure_inner(d);
    unsafe {
        (*inner).refcount.fetch_add(1, Ordering::Relaxed);
    }
    d
}

#[unsafe(no_mangle)]
pub extern "C" fn put_device(d: *mut DeviceShim) {
    if d.is_null() {
        return;
    }
    let inner = unsafe { (*d).inner };
    if inner.is_null() {
        return;
    }
    let prev = unsafe { (*inner).refcount.fetch_sub(1, Ordering::AcqRel) };
    if prev == 1 {
        // K3: don't free the inner — the device struct is module-
        // owned, freeing it would dangle the user's pointer.  Leak
        // is harmless for K3 since modules aren't unloaded.
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn dev_set_drvdata(d: *mut DeviceShim, data: *mut c_void) {
    if d.is_null() {
        return;
    }
    unsafe {
        (*d).driver_data = data;
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn dev_get_drvdata(d: *const DeviceShim) -> *mut c_void {
    if d.is_null() {
        return core::ptr::null_mut();
    }
    unsafe { (*d).driver_data }
}

ksym!(device_initialize);
ksym!(device_add);
ksym!(device_register);
ksym!(device_unregister);
ksym!(get_device);
ksym!(put_device);
ksym!(dev_set_drvdata);
ksym!(dev_get_drvdata);
