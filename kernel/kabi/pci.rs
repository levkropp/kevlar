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

// ── K18 additions (bochs surface) ─────────────────────────────

/// `__devm_request_region(parent, start, n, name)` — request a
/// resource region.  K18: no-op returning a non-null cookie so
/// callers don't treat it as failure.  Real region tracking is
/// K20+.
#[unsafe(no_mangle)]
pub extern "C" fn __devm_request_region(
    _dev: *mut c_void,
    _parent: *mut c_void,
    _start: u64,
    _n: u64,
    _name: *const c_char,
) -> *mut c_void {
    // Return any non-null sentinel; bochs only checks for null.
    1 as *mut c_void
}

#[unsafe(no_mangle)]
pub extern "C" fn devm_ioremap_wc(
    _dev: *mut c_void,
    _addr: u64,
    _size: usize,
) -> *mut c_void {
    core::ptr::null_mut()
}

ksym!(__devm_request_region);
ksym!(devm_ioremap_wc);

// `iomem_resource` and `ioport_resource` are the system-wide
// trees Linux uses to track IO memory and port-IO regions.
// K18 exposes 64-byte zero buffers — nothing dereferences fields
// at K18 since probe doesn't fire.

#[unsafe(no_mangle)]
pub static iomem_resource: [u8; 64] = [0; 64];
crate::ksym_static!(iomem_resource);

#[unsafe(no_mangle)]
pub static ioport_resource: [u8; 64] = [0; 64];
crate::ksym_static!(ioport_resource);
