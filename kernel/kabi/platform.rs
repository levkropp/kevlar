// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux `platform_bus_type` + `platform_device` + `platform_driver`
//! shims for K3.  Provides the canonical "register a driver, watch
//! it bind to a device by name" path.

use core::ffi::{c_char, c_void};

use crate::kabi::bus::{add_device, add_driver, BusInner, BusTypeShim, DeviceDriverShim};
use crate::kabi::device::{ensure_inner as ensure_device_inner, DeviceShim};
use crate::{ksym, ksym_static};

#[repr(C)]
pub struct PlatformDeviceShim {
    pub name: *const c_char,
    pub id: i32,
    pub dev: DeviceShim,
    pub inner: *mut (),
}

#[repr(C)]
pub struct PlatformDriverShim {
    pub probe: Option<extern "C" fn(*mut PlatformDeviceShim) -> i32>,
    pub remove: Option<extern "C" fn(*mut PlatformDeviceShim) -> i32>,
    pub driver: DeviceDriverShim,
}

// Storage for the kernel-provided platform bus.  `BUS_INNER` is a
// `static` so its address is stable for the entire kernel lifetime;
// `PLATFORM_BUS` carries the pointer to it via the type's
// `inner` field.

static mut PLATFORM_BUS_INNER: BusInner = BusInner::new();

/// Name string for the platform bus.  Kept as a static byte array
/// so it has stable address.
static PLATFORM_BUS_NAME: [u8; 9] = *b"platform\0";

#[unsafe(no_mangle)]
pub static mut platform_bus_type: BusTypeShim = BusTypeShim {
    name: PLATFORM_BUS_NAME.as_ptr() as *const c_char,
    match_fn: Some(platform_match),
    inner: core::ptr::null_mut(),
};

ksym_static!(platform_bus_type);

/// Initialize the platform bus.  Idempotent.  Called from
/// `kabi::init()` once the kABI runtime is up.
pub fn init() {
    unsafe {
        if platform_bus_type.inner.is_null() {
            platform_bus_type.inner =
                &raw mut PLATFORM_BUS_INNER as *mut BusInner;
        }
    }
}

/// Bus match callback.  Linux's match chain is
/// driver_override → of → acpi → id_table → name strcmp.
/// K3 implements only the last step.
extern "C" fn platform_match(
    dev: *mut DeviceShim,
    drv: *const DeviceDriverShim,
) -> i32 {
    if dev.is_null() || drv.is_null() {
        return 0;
    }
    // The DeviceShim is embedded inside a PlatformDeviceShim at a
    // known offset (after the `name` and `id` fields).
    let pdev = pdev_of_dev(dev);
    if pdev.is_null() {
        return 0;
    }
    let pdev_name = unsafe { (*pdev).name };
    let drv_name = unsafe { (*drv).name };
    if pdev_name.is_null() || drv_name.is_null() {
        return 0;
    }
    if c_str_eq(pdev_name, drv_name) {
        1
    } else {
        0
    }
}

/// Recover the containing `PlatformDeviceShim` from its embedded
/// `DeviceShim`.  Pure offset arithmetic — `dev` is at offset
/// `offsetof(PlatformDeviceShim, dev)`.
fn pdev_of_dev(dev: *mut DeviceShim) -> *mut PlatformDeviceShim {
    let off = core::mem::offset_of!(PlatformDeviceShim, dev);
    unsafe { (dev as *mut u8).sub(off) as *mut PlatformDeviceShim }
}

/// Recover the containing `PlatformDriverShim` from its embedded
/// `DeviceDriverShim`.
fn pdrv_of_drv(drv: *mut DeviceDriverShim) -> *mut PlatformDriverShim {
    let off = core::mem::offset_of!(PlatformDriverShim, driver);
    unsafe { (drv as *mut u8).sub(off) as *mut PlatformDriverShim }
}

/// Probe thunk: when the bus matches, it calls `drv->probe(dev)` —
/// but `drv` here is the `device_driver` handle inside a
/// `platform_driver`, and the module's `pdrv->probe` expects a
/// `platform_device *`.  This thunk recovers both and forwards.
extern "C" fn platform_drv_probe(dev: *mut DeviceShim) -> i32 {
    let pdev = pdev_of_dev(dev);
    let drv = unsafe { (*dev).driver };
    if drv.is_null() {
        return -22;
    }
    let pdrv = pdrv_of_drv(drv);
    if let Some(probe) = unsafe { (*pdrv).probe } {
        return probe(pdev);
    }
    0
}

extern "C" fn platform_drv_remove(dev: *mut DeviceShim) -> i32 {
    let pdev = pdev_of_dev(dev);
    let drv = unsafe { (*dev).driver };
    if drv.is_null() {
        return 0;
    }
    let pdrv = pdrv_of_drv(drv);
    if let Some(rem) = unsafe { (*pdrv).remove } {
        return rem(pdev);
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn platform_device_register(
    pdev: *mut PlatformDeviceShim,
) -> i32 {
    if pdev.is_null() {
        return -22;
    }
    init(); // ensure platform_bus_type.inner is populated
    unsafe {
        let dev = &raw mut (*pdev).dev;
        ensure_device_inner(dev);
        (*dev).bus = &raw const platform_bus_type;
        (*dev).init_name = (*pdev).name;
        add_device(&raw const platform_bus_type, dev);
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn platform_device_unregister(
    pdev: *mut PlatformDeviceShim,
) {
    if pdev.is_null() {
        return;
    }
    unsafe {
        let dev = &raw mut (*pdev).dev;
        crate::kabi::bus::remove_device(&raw const platform_bus_type, dev);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn platform_driver_register(
    pdrv: *mut PlatformDriverShim,
) -> i32 {
    if pdrv.is_null() {
        return -22;
    }
    init();
    unsafe {
        let drv = &raw mut (*pdrv).driver;
        (*drv).bus = &raw const platform_bus_type;
        // Replace the driver's probe/remove with our thunks; the
        // bus calls `drv->probe(dev)` and the thunk forwards.
        (*drv).probe = Some(platform_drv_probe);
        (*drv).remove = Some(platform_drv_remove);
        add_driver(&raw const platform_bus_type, drv);
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn platform_driver_unregister(
    pdrv: *mut PlatformDriverShim,
) {
    if pdrv.is_null() {
        return;
    }
    unsafe {
        let drv = &raw mut (*pdrv).driver;
        crate::kabi::bus::remove_driver(&raw const platform_bus_type, drv);
    }
}

/// Linux's `__platform_driver_register` is the canonical name; the
/// `platform_driver_register` symbol is itself a #define wrapping
/// it with a THIS_MODULE arg.  K2/K3 honor both.
#[unsafe(no_mangle)]
pub extern "C" fn __platform_driver_register(
    pdrv: *mut PlatformDriverShim,
    _module: *const (),
) -> i32 {
    platform_driver_register(pdrv)
}

#[unsafe(no_mangle)]
pub extern "C" fn platform_set_drvdata(
    pdev: *mut PlatformDeviceShim,
    data: *mut c_void,
) {
    if pdev.is_null() {
        return;
    }
    unsafe {
        (*pdev).dev.driver_data = data;
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn platform_get_drvdata(
    pdev: *const PlatformDeviceShim,
) -> *mut c_void {
    if pdev.is_null() {
        return core::ptr::null_mut();
    }
    unsafe { (*pdev).dev.driver_data }
}

ksym!(platform_device_register);
ksym!(platform_device_unregister);
ksym!(platform_driver_register);
ksym!(platform_driver_unregister);
ksym!(__platform_driver_register);
ksym!(platform_set_drvdata);
ksym!(platform_get_drvdata);

#[inline]
fn c_str_eq(a: *const c_char, b: *const c_char) -> bool {
    let mut i = 0usize;
    loop {
        let av = unsafe { *a.add(i) };
        let bv = unsafe { *b.add(i) };
        if av != bv {
            return false;
        }
        if av == 0 {
            return true;
        }
        i += 1;
        if i > 256 {
            return false;
        }
    }
}
