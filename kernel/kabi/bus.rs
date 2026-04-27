// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux `struct bus_type` + `struct device_driver` shims for K3.
//!
//! K3 only supports kernel-provided buses (e.g. `platform_bus_type`
//! in `kernel/kabi/platform.rs`).  `bus_register` is exported for
//! API surface but doesn't drive any new behavior at this milestone.
//!
//! The match algorithm: when a driver registers, walk every device
//! currently on the same bus and call `bus->match(dev, drv)`; on
//! return value != 0, set `dev.driver = drv` and call `drv->probe`.
//! When a device registers, do the symmetric walk.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ffi::c_char;

use kevlar_platform::spinlock::SpinLock;

use crate::kabi::device::DeviceShim;
use crate::ksym;

#[repr(C)]
pub struct DeviceDriverShim {
    pub name: *const c_char,
    pub bus: *const BusTypeShim,
    pub probe: Option<extern "C" fn(*mut DeviceShim) -> i32>,
    pub remove: Option<extern "C" fn(*mut DeviceShim) -> i32>,
    pub inner: *mut (),
}

#[repr(C)]
pub struct BusTypeShim {
    pub name: *const c_char,
    pub match_fn: Option<
        extern "C" fn(*mut DeviceShim, *const DeviceDriverShim) -> i32,
    >,
    pub inner: *mut BusInner,
}

pub struct BusInner {
    pub drivers: SpinLock<Vec<*mut DeviceDriverShim>>,
    pub devices: SpinLock<Vec<*mut DeviceShim>>,
}

impl BusInner {
    pub const fn new() -> Self {
        Self {
            drivers: SpinLock::new(Vec::new()),
            devices: SpinLock::new(Vec::new()),
        }
    }
}

// SAFETY: these pointers are accessed only behind the bus's
// spinlocks; the values are kernel-/module-stable.
unsafe impl Send for BusInner {}
unsafe impl Sync for BusInner {}

pub fn add_driver(bus: *const BusTypeShim, drv: *mut DeviceDriverShim) {
    if bus.is_null() || drv.is_null() {
        return;
    }
    let inner = unsafe { (*bus).inner };
    if inner.is_null() {
        log::warn!("kabi: bus has null inner; was bus_register called?");
        return;
    }
    {
        let mut lst = unsafe { (*inner).drivers.lock() };
        if lst.iter().any(|p| *p == drv) {
            return;
        }
        lst.push(drv);
    }
    // Walk devices, attempt match.
    let snapshot: Vec<*mut DeviceShim> = unsafe {
        (*inner).devices.lock().clone()
    };
    for dev in snapshot {
        let mr = unsafe { (*bus).match_fn };
        if let Some(m) = mr {
            if m(dev, drv) != 0 {
                bind(dev, drv);
            }
        }
    }
}

pub fn remove_driver(bus: *const BusTypeShim, drv: *mut DeviceDriverShim) {
    if bus.is_null() || drv.is_null() {
        return;
    }
    let inner = unsafe { (*bus).inner };
    if inner.is_null() {
        return;
    }
    {
        let mut lst = unsafe { (*inner).drivers.lock() };
        if let Some(pos) = lst.iter().position(|p| *p == drv) {
            lst.remove(pos);
        }
    }
    // Unbind any devices currently bound to this driver.
    let devs: Vec<*mut DeviceShim> =
        unsafe { (*inner).devices.lock().clone() };
    for dev in devs {
        if unsafe { (*dev).driver } == drv {
            if let Some(rem) = unsafe { (*drv).remove } {
                let _ = rem(dev);
            }
            unsafe {
                (*dev).driver = core::ptr::null_mut();
            }
        }
    }
}

pub fn add_device(bus: *const BusTypeShim, dev: *mut DeviceShim) {
    if bus.is_null() || dev.is_null() {
        return;
    }
    let inner = unsafe { (*bus).inner };
    if inner.is_null() {
        log::warn!("kabi: add_device on bus with null inner");
        return;
    }
    {
        let mut lst = unsafe { (*inner).devices.lock() };
        if lst.iter().any(|p| *p == dev) {
            return;
        }
        lst.push(dev);
    }
    // Walk drivers, attempt match.
    let snapshot: Vec<*mut DeviceDriverShim> = unsafe {
        (*inner).drivers.lock().clone()
    };
    for drv in snapshot {
        let mr = unsafe { (*bus).match_fn };
        if let Some(m) = mr {
            if m(dev, drv) != 0 {
                bind(dev, drv);
                break;  // Linux: one driver per device.
            }
        }
    }
}

pub fn remove_device(bus: *const BusTypeShim, dev: *mut DeviceShim) {
    if bus.is_null() || dev.is_null() {
        return;
    }
    let inner = unsafe { (*bus).inner };
    if inner.is_null() {
        return;
    }
    {
        let mut lst = unsafe { (*inner).devices.lock() };
        if let Some(pos) = lst.iter().position(|p| *p == dev) {
            lst.remove(pos);
        }
    }
    let drv = unsafe { (*dev).driver };
    if !drv.is_null() {
        if let Some(rem) = unsafe { (*drv).remove } {
            let _ = rem(dev);
        }
        unsafe {
            (*dev).driver = core::ptr::null_mut();
        }
    }
}

fn bind(dev: *mut DeviceShim, drv: *mut DeviceDriverShim) {
    unsafe {
        (*dev).driver = drv;
    }
    if let Some(probe) = unsafe { (*drv).probe } {
        let rc = probe(dev);
        if rc != 0 {
            log::warn!("kabi: probe returned {} — leaving device unbound", rc);
            unsafe {
                (*dev).driver = core::ptr::null_mut();
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn driver_register(drv: *mut DeviceDriverShim) -> i32 {
    if drv.is_null() {
        return -22;
    }
    let bus = unsafe { (*drv).bus };
    if bus.is_null() {
        log::warn!("kabi: driver_register with null bus");
        return -22;
    }
    add_driver(bus, drv);
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn driver_unregister(drv: *mut DeviceDriverShim) {
    if drv.is_null() {
        return;
    }
    let bus = unsafe { (*drv).bus };
    if bus.is_null() {
        return;
    }
    remove_driver(bus, drv);
}

/// Modules that introduce their own bus type call this to allocate
/// the per-bus `BusInner`.  K3's bundled `platform_bus_type` is
/// pre-initialized in `kabi::init()` so the demo doesn't need this.
#[unsafe(no_mangle)]
pub extern "C" fn bus_register(bus: *mut BusTypeShim) -> i32 {
    if bus.is_null() {
        return -22;
    }
    if unsafe { (*bus).inner.is_null() } {
        let boxed = Box::new(BusInner::new());
        unsafe {
            (*bus).inner = Box::into_raw(boxed);
        }
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn bus_unregister(_bus: *mut BusTypeShim) {
    // K3: leak the BusInner.
}

ksym!(driver_register);
ksym!(driver_unregister);
ksym!(bus_register);
ksym!(bus_unregister);
