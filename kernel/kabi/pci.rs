// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! PCI bus shim — registration + minimal device-managed helpers.
//!
//! `__pci_register_driver` is the only one whose return value
//! matters at K17: cirrus-qemu's `init_module` calls it, and the
//! return code is the init_module return code.  We log + return 0
//! (success).  No PCI bus walking; probe never fires; the
//! driver is "registered" but never matched.  Same shape as
//! K12's `__register_virtio_driver`.
//!
//! K20+ when real PCI bus discovery lands, this gets a proper
//! driver-list-walking implementation.

use core::ffi::{c_char, c_void};

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn __pci_register_driver(
    _drv: *mut c_void,
    _owner: *mut c_void,
    _mod_name: *const c_char,
) -> i32 {
    log::info!("kabi: __pci_register_driver (stub)");
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn pci_unregister_driver(_drv: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn pcim_enable_device(_dev: *mut c_void) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn pcim_request_all_regions(
    _dev: *mut c_void,
    _name: *const c_char,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn aperture_remove_conflicting_pci_devices(
    _dev: *mut c_void,
    _name: *const c_char,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn devm_ioremap(
    _dev: *mut c_void,
    _addr: u64,
    _size: usize,
) -> *mut c_void {
    core::ptr::null_mut()
}

/// `video_firmware_drivers_only()` — Linux predicate gating
/// firmware-only video driver loads (controlled by
/// `video=firmware` cmdline).  K17: always false.
#[unsafe(no_mangle)]
pub extern "C" fn video_firmware_drivers_only() -> bool {
    false
}

ksym!(__pci_register_driver);
ksym!(pci_unregister_driver);
ksym!(pcim_enable_device);
ksym!(pcim_request_all_regions);
ksym!(aperture_remove_conflicting_pci_devices);
ksym!(devm_ioremap);
ksym!(video_firmware_drivers_only);
