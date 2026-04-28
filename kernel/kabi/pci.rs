// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! PCI bus shim — driver registration + bus walking + probe firing.
//!
//! K19: real `__pci_register_driver` records the driver in a list,
//! then walks the static fake-device list, matches on (vendor,
//! device), and calls `probe()`.  This is the **first probe-firing
//! milestone of the kABI arc** — a real Ubuntu kernel module's
//! callback runs against fields we control.
//!
//! Layout assumptions verified by disassembly of cirrus-qemu's
//! `cirrus_pci_driver` static:
//!
//! - `struct pci_driver`: name @ +0, id_table @ +8, probe @ +16,
//!   remove @ +24.
//! - `struct pci_device_id`: vendor @ +0, device @ +4, subvendor @ +8,
//!   subdevice @ +12, class @ +16, class_mask @ +20, driver_data @ +24,
//!   override_only @ +32.  Total size 40.  End-of-table sentinel:
//!   `vendor == 0`.
//!
//! For the fake `pci_dev` buffer, cirrus's probe reads:
//! - `&pdev->dev` at +208 (the embedded `struct device`, used as
//!   `parent` for `__devm_drm_dev_alloc`).
//! - `pdev->resource[0].{start,end}` at +1024 / +1032.
//! - `pdev->resource[2].{start,end}` at +1088 / +1096.

use alloc::vec::Vec;
use core::ffi::{c_char, c_void};

use kevlar_platform::spinlock::SpinLock;

use crate::ksym;

// ── Layout constants from cirrus-qemu disasm ──────────────────

const PCI_DRIVER_OFF_NAME: usize = 0;
const PCI_DRIVER_OFF_ID_TABLE: usize = 8;
const PCI_DRIVER_OFF_PROBE: usize = 16;

const PCI_DEVICE_ID_SIZE: usize = 40;
const PCI_DEVICE_ID_OFF_VENDOR: usize = 0;
const PCI_DEVICE_ID_OFF_DEVICE: usize = 4;

const PDEV_OFF_RESOURCE0_START: usize = 1024;
const PDEV_OFF_RESOURCE0_END: usize = 1032;
const PDEV_OFF_RESOURCE2_START: usize = 1088;
const PDEV_OFF_RESOURCE2_END: usize = 1096;
const PDEV_BUF_SIZE: usize = 4096;

// ── Driver registry ──────────────────────────────────────────

#[derive(Clone, Copy)]
struct RegisteredDriver {
    /// `struct pci_driver *` from the registering module.
    drv: usize,
    /// Driver name (string in module memory).  Held for log only.
    name_ptr: *const c_char,
}

// SAFETY: the addresses are stable for the lifetime of the loaded
// module; we never dereference them on cross-thread paths in K19.
unsafe impl Send for RegisteredDriver {}

static REGISTERED: SpinLock<Vec<RegisteredDriver>> =
    SpinLock::new(Vec::new());

// ── Fake PCI device backing memory ────────────────────────────

struct FakePciDev {
    vendor: u16,
    device: u16,
    /// 4KB buffer that probe sees as `struct pci_dev *`.
    pdev_buf: *mut u8,
    /// PA we report for resource[0].start; VA we return from
    /// devm_ioremap when called with that PA.
    bar0_pa: u64,
    bar0_va: usize,
    bar0_size: usize,
    /// Same for resource[2] (BAR2 — cirrus mmio regs).
    bar2_pa: u64,
    bar2_va: usize,
    bar2_size: usize,
}

unsafe impl Send for FakePciDev {}

static FAKE_DEVICES: SpinLock<Vec<FakePciDev>> = SpinLock::new(Vec::new());

/// Lazy initializer for the fake Cirrus PCI device.  Allocates:
/// - A 4KB pdev buffer (zeroed).
/// - A 16MB BAR0 (fake VRAM).
/// - A 4KB BAR2 (fake mmio regs).
///
/// Then writes resource[0/2] start/end into the pdev buffer at the
/// known offsets.  PAs are synthesized as the buffer addresses
/// themselves (we don't have real BAR addresses; the lookup table
/// in `devm_ioremap` translates by exact match).
fn ensure_cirrus_fake_device() {
    let mut devices = FAKE_DEVICES.lock();
    if !devices.is_empty() {
        return;
    }
    drop(devices);

    let pdev_buf = super::alloc::kzalloc(PDEV_BUF_SIZE, 0) as *mut u8;
    // Small fake BARs at K19 — cirrus's probe stores the ioremap'd
    // pointers in drm_dev fields but doesn't read/write them at
    // probe time.  4 KB each is plenty.
    let bar0_size: usize = 4096;
    let bar2_size: usize = 4096;
    let bar0_va = super::alloc::kzalloc(bar0_size, 0) as usize;
    let bar2_va = super::alloc::kzalloc(bar2_size, 0) as usize;
    if pdev_buf.is_null() || bar0_va == 0 || bar2_va == 0 {
        log::warn!("kabi: failed to allocate fake cirrus PCI device backing");
        return;
    }

    // Synthesize PA values that won't collide with anything real.
    // Choose high addresses (>= 4 GB) as plausible PCI BARs.
    let bar0_pa: u64 = 0x1_0000_0000;
    let bar2_pa: u64 = 0x1_1000_0000;

    // Write resource[0].start/end
    unsafe {
        core::ptr::write_unaligned(
            pdev_buf.add(PDEV_OFF_RESOURCE0_START) as *mut u64,
            bar0_pa,
        );
        core::ptr::write_unaligned(
            pdev_buf.add(PDEV_OFF_RESOURCE0_END) as *mut u64,
            bar0_pa + bar0_size as u64 - 1,
        );
        core::ptr::write_unaligned(
            pdev_buf.add(PDEV_OFF_RESOURCE2_START) as *mut u64,
            bar2_pa,
        );
        core::ptr::write_unaligned(
            pdev_buf.add(PDEV_OFF_RESOURCE2_END) as *mut u64,
            bar2_pa + bar2_size as u64 - 1,
        );
    }

    let mut devices = FAKE_DEVICES.lock();
    devices.push(FakePciDev {
        vendor: 0x1013,
        device: 0x00B8,
        pdev_buf,
        bar0_pa,
        bar0_va,
        bar0_size,
        bar2_pa,
        bar2_va,
        bar2_size,
    });
    log::info!(
        "kabi: registered fake PCI device 0x1013:0x00B8 (Cirrus VGA), \
         pdev_buf={:#x}, BAR0_pa={:#x} VA={:#x}, BAR2_pa={:#x} VA={:#x}",
        pdev_buf as usize,
        bar0_pa,
        bar0_va,
        bar2_pa,
        bar2_va,
    );
}

/// `devm_ioremap` translates a fake-PA back to its backing VA.
/// Used by cirrus's probe when ioremap'ing each BAR.
fn lookup_fake_bar_va(addr: u64) -> Option<usize> {
    let devices = FAKE_DEVICES.lock();
    for d in devices.iter() {
        if addr >= d.bar0_pa && addr < d.bar0_pa + d.bar0_size as u64 {
            return Some(d.bar0_va + (addr - d.bar0_pa) as usize);
        }
        if addr >= d.bar2_pa && addr < d.bar2_pa + d.bar2_size as u64 {
            return Some(d.bar2_va + (addr - d.bar2_pa) as usize);
        }
    }
    None
}

// ── Bus walking ──────────────────────────────────────────────

fn match_id_table(id_table: usize, vendor: u16, device: u16) -> Option<usize> {
    if id_table == 0 {
        return None;
    }
    let mut idx: usize = 0;
    loop {
        let entry_addr = id_table + idx * PCI_DEVICE_ID_SIZE;
        let v = unsafe {
            core::ptr::read_unaligned(
                (entry_addr + PCI_DEVICE_ID_OFF_VENDOR) as *const u32,
            )
        } as u16;
        let d = unsafe {
            core::ptr::read_unaligned(
                (entry_addr + PCI_DEVICE_ID_OFF_DEVICE) as *const u32,
            )
        } as u16;
        // End-of-table sentinel.
        if v == 0 && d == 0 {
            return None;
        }
        if v == vendor && d == device {
            return Some(entry_addr);
        }
        idx += 1;
        if idx > 64 {
            // Defensive: a malformed table shouldn't loop forever.
            log::warn!("kabi: pci id_table walk exceeded 64 entries; aborting");
            return None;
        }
    }
}

/// Walk all registered drivers + all fake devices; for each match,
/// invoke `probe(pdev, matched_id)` and log the return.
pub fn walk_and_probe() {
    ensure_cirrus_fake_device();
    let drivers = REGISTERED.lock().clone();
    let devices: Vec<*mut u8> = FAKE_DEVICES
        .lock()
        .iter()
        .map(|d| d.pdev_buf)
        .collect();
    let dev_ids: Vec<(u16, u16)> = FAKE_DEVICES
        .lock()
        .iter()
        .map(|d| (d.vendor, d.device))
        .collect();

    log::info!(
        "kabi: PCI walk: {} driver(s), {} device(s)",
        drivers.len(),
        devices.len()
    );

    for d in drivers.iter() {
        let id_table = unsafe {
            core::ptr::read_unaligned((d.drv + PCI_DRIVER_OFF_ID_TABLE) as *const usize)
        };
        let probe_fn = unsafe {
            core::ptr::read_unaligned((d.drv + PCI_DRIVER_OFF_PROBE) as *const usize)
        };
        let drv_name = if d.name_ptr.is_null() {
            "?"
        } else {
            unsafe { core_cstr_to_str(d.name_ptr).unwrap_or("?") }
        };
        if probe_fn == 0 {
            log::warn!("kabi: PCI walk: driver '{}' has no probe; skipping", drv_name);
            continue;
        }

        for ((dev_idx, pdev), &(vendor, device)) in
            devices.iter().enumerate().zip(dev_ids.iter())
        {
            let _ = dev_idx;
            if let Some(matched_id) = match_id_table(id_table, vendor, device) {
                log::info!(
                    "kabi: PCI walk: probing driver '{}' against {:04x}:{:04x}",
                    drv_name,
                    vendor,
                    device
                );
                let probe: extern "C" fn(*mut c_void, *const c_void) -> i32 =
                    unsafe { core::mem::transmute(probe_fn) };
                let rc = probe(*pdev as *mut c_void, matched_id as *const c_void);
                log::info!(
                    "kabi: PCI walk: '{}' probe returned {}",
                    drv_name,
                    rc
                );
            }
        }
    }
}

unsafe fn core_cstr_to_str(p: *const c_char) -> Option<&'static str> {
    if p.is_null() {
        return None;
    }
    let mut len = 0usize;
    while unsafe { *p.add(len) } != 0 && len < 64 {
        len += 1;
    }
    let bytes = unsafe { core::slice::from_raw_parts(p as *const u8, len) };
    core::str::from_utf8(bytes).ok()
}

// ── kABI entry points ────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn __pci_register_driver(
    drv: *mut c_void,
    _owner: *mut c_void,
    mod_name: *const c_char,
) -> i32 {
    let drv_addr = drv as usize;
    let name_ptr = unsafe {
        core::ptr::read_unaligned(
            (drv_addr + PCI_DRIVER_OFF_NAME) as *const *const c_char,
        )
    };
    log::info!(
        "kabi: __pci_register_driver: name={:?} mod={:?}",
        unsafe { core_cstr_to_str(name_ptr) },
        unsafe { core_cstr_to_str(mod_name) }
    );
    REGISTERED.lock().push(RegisteredDriver {
        drv: drv_addr,
        name_ptr,
    });
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
    addr: u64,
    _size: usize,
) -> *mut c_void {
    if let Some(va) = lookup_fake_bar_va(addr) {
        return va as *mut c_void;
    }
    log::warn!("kabi: devm_ioremap({:#x}) — no fake BAR mapping", addr);
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn devm_ioremap_wc(
    dev: *mut c_void,
    addr: u64,
    size: usize,
) -> *mut c_void {
    devm_ioremap(dev, addr, size)
}

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
ksym!(devm_ioremap_wc);
ksym!(video_firmware_drivers_only);

// ── K18 surface (still link-only at K19) ──────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn __devm_request_region(
    _dev: *mut c_void,
    _parent: *mut c_void,
    _start: u64,
    _n: u64,
    _name: *const c_char,
) -> *mut c_void {
    1 as *mut c_void
}

ksym!(__devm_request_region);

#[unsafe(no_mangle)]
pub static iomem_resource: [u8; 64] = [0; 64];
crate::ksym_static!(iomem_resource);

#[unsafe(no_mangle)]
pub static ioport_resource: [u8; 64] = [0; 64];
crate::ksym_static!(ioport_resource);
