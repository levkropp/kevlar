// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux virtio bus core — driver registration + bus walking +
//! probe firing (K23).
//!
//! K12 introduced `__register_virtio_driver` as a "log + return 0"
//! stub.  K23 turns it into a real bus implementation that mirrors
//! K19's PCI walker:
//!
//! 1. Track registered drivers in `REGISTERED`.
//! 2. Statically declare a fake virtio_input device.
//! 3. Walk fake devices on driver register, match `device_id`
//!    against the driver's `id_table`, call `probe(vdev)`.
//!
//! Layout assumptions verified by disassembly of virtio_input.ko's
//! `virtio_input_driver` static:
//!
//! - `struct virtio_driver`: id_table @ +152, probe @ +200.
//!   (struct device_driver is the leading ~120 bytes, then
//!   id_table, feature_table, etc.)
//! - `struct virtio_device_id`: 8 bytes (device:u32, vendor:u32),
//!   end-of-table sentinel `device=0`.
//! - `struct virtio_device`: config @ +832, features @ +872, priv
//!   @ +888.
//! - `struct virtio_config_ops`: find_vqs @ +48 (called in
//!   virtinput_init_vqs).

use alloc::vec::Vec;
use core::ffi::{c_char, c_void};

use kevlar_platform::spinlock::SpinLock;

use crate::ksym;

// ── Layout constants ──────────────────────────────────────────

const VIRTIO_DRIVER_OFF_ID_TABLE: usize = 152;
const VIRTIO_DRIVER_OFF_PROBE: usize = 200;

const VIRTIO_DEVICE_ID_SIZE: usize = 8;
const VIRTIO_DEVICE_ID_OFF_DEVICE: usize = 0;
const VIRTIO_DEVICE_ID_OFF_VENDOR: usize = 4;

// struct virtio_device offsets (only the fields virtinput touches)
const VDEV_OFF_CONFIG: usize = 832;
const VDEV_OFF_FEATURES: usize = 872;
const VDEV_OFF_PRIV: usize = 888;
const VDEV_BUF_SIZE: usize = 2048;

// struct virtio_config_ops slots (offsets of function pointers)
const CFG_OFF_GET: usize = 0;
const CFG_OFF_SET: usize = 8;
const CFG_OFF_GENERATION: usize = 16;
const CFG_OFF_GET_STATUS: usize = 24;
const CFG_OFF_SET_STATUS: usize = 32;
const CFG_OFF_RESET: usize = 40;
const CFG_OFF_FIND_VQS: usize = 48;
const CFG_OFF_DEL_VQS: usize = 56;
const CFG_OFF_SYNCHRONIZE_CBS: usize = 64;
const CFG_OFF_GET_FEATURES: usize = 72;
const CFG_OFF_FINALIZE_FEATURES: usize = 80;
const CFG_BUF_SIZE: usize = 256;

const VIRTIO_ID_INPUT: u32 = 18;
const VIRTIO_ANY_ID: u32 = 0xFFFF_FFFF;
const VIRTIO_F_VERSION_1: u64 = 1u64 << 32;

// ── Driver registry ──────────────────────────────────────────

#[derive(Clone, Copy)]
struct RegisteredVirtioDriver {
    drv: usize,
    name_ptr: *const c_char,
}

unsafe impl Send for RegisteredVirtioDriver {}

static REGISTERED: SpinLock<Vec<RegisteredVirtioDriver>> =
    SpinLock::new(Vec::new());

// ── Fake virtio device + config_ops ───────────────────────────

struct FakeVirtioDev {
    device_id: u32,
    vendor_id: u32,
    vdev_buf: *mut u8,
}

unsafe impl Send for FakeVirtioDev {}

static FAKE_DEVICES: SpinLock<Vec<FakeVirtioDev>> = SpinLock::new(Vec::new());

// Fake config_ops vtable: function pointers at the right offsets
// matching struct virtio_config_ops.
extern "C" fn fake_get(
    _vdev: *mut c_void,
    _offset: u32,
    buf: *mut c_void,
    len: u32,
) {
    if !buf.is_null() && len > 0 {
        unsafe { core::ptr::write_bytes(buf as *mut u8, 0, len as usize); }
    }
}

extern "C" fn fake_set(
    _vdev: *mut c_void,
    _offset: u32,
    _buf: *const c_void,
    _len: u32,
) {
}

extern "C" fn fake_generation(_vdev: *mut c_void) -> u32 {
    0
}

extern "C" fn fake_get_status(_vdev: *mut c_void) -> u8 {
    0
}

extern "C" fn fake_set_status(_vdev: *mut c_void, _status: u8) {}

extern "C" fn fake_reset(_vdev: *mut c_void) {}

/// `find_vqs(vdev, nvqs, vqs[], callbacks[], names[], ctx, desc)`
/// Linux 7.0 signature (the exact arg order).  We populate
/// `vqs[0..nvqs]` with fake virtqueue pointers and return 0.
extern "C" fn fake_find_vqs(
    _vdev: *mut c_void,
    nvqs: u32,
    vqs: *mut *mut c_void,
    _callbacks: *mut *mut c_void,
    _names: *const *const c_char,
    _ctx: *const bool,
    _desc: *mut c_void,
) -> i32 {
    let fake_vqs = FAKE_VIRTQUEUES.lock();
    for i in 0..(nvqs as usize) {
        let vq = if i < fake_vqs.0.len() {
            fake_vqs.0[i] as *mut c_void
        } else {
            core::ptr::null_mut()
        };
        unsafe { *vqs.add(i) = vq; }
    }
    0
}

extern "C" fn fake_del_vqs(_vdev: *mut c_void) {}

extern "C" fn fake_synchronize_cbs(_vdev: *mut c_void) {}

extern "C" fn fake_get_features(_vdev: *mut c_void) -> u64 {
    VIRTIO_F_VERSION_1
}

extern "C" fn fake_finalize_features(_vdev: *mut c_void) -> i32 {
    0
}

struct FakeVqs(Vec<usize>);
unsafe impl Send for FakeVqs {}

static FAKE_VIRTQUEUES: SpinLock<FakeVqs> = SpinLock::new(FakeVqs(Vec::new()));

/// Lazy-init fake virtio_input device + fake config_ops vtable.
fn ensure_fake_virtio_device() {
    let mut devices = FAKE_DEVICES.lock();
    if !devices.is_empty() {
        return;
    }
    drop(devices);

    // Allocate two fake virtqueues (event_vq + status_vq).
    let mut vqs = FAKE_VIRTQUEUES.lock();
    if vqs.0.is_empty() {
        for _ in 0..2 {
            let vq = super::alloc::kzalloc(4096, 0);
            if !vq.is_null() {
                vqs.0.push(vq as usize);
            }
        }
    }
    drop(vqs);

    // Build fake config_ops vtable in a heap buffer at the right
    // offsets.
    let cfg_buf = super::alloc::kzalloc(CFG_BUF_SIZE, 0) as *mut u8;
    if cfg_buf.is_null() {
        log::warn!("kabi: failed to allocate fake virtio config_ops");
        return;
    }
    unsafe {
        core::ptr::write(
            cfg_buf.add(CFG_OFF_GET) as *mut usize,
            fake_get as usize,
        );
        core::ptr::write(
            cfg_buf.add(CFG_OFF_SET) as *mut usize,
            fake_set as usize,
        );
        core::ptr::write(
            cfg_buf.add(CFG_OFF_GENERATION) as *mut usize,
            fake_generation as usize,
        );
        core::ptr::write(
            cfg_buf.add(CFG_OFF_GET_STATUS) as *mut usize,
            fake_get_status as usize,
        );
        core::ptr::write(
            cfg_buf.add(CFG_OFF_SET_STATUS) as *mut usize,
            fake_set_status as usize,
        );
        core::ptr::write(
            cfg_buf.add(CFG_OFF_RESET) as *mut usize,
            fake_reset as usize,
        );
        core::ptr::write(
            cfg_buf.add(CFG_OFF_FIND_VQS) as *mut usize,
            fake_find_vqs as usize,
        );
        core::ptr::write(
            cfg_buf.add(CFG_OFF_DEL_VQS) as *mut usize,
            fake_del_vqs as usize,
        );
        core::ptr::write(
            cfg_buf.add(CFG_OFF_SYNCHRONIZE_CBS) as *mut usize,
            fake_synchronize_cbs as usize,
        );
        core::ptr::write(
            cfg_buf.add(CFG_OFF_GET_FEATURES) as *mut usize,
            fake_get_features as usize,
        );
        core::ptr::write(
            cfg_buf.add(CFG_OFF_FINALIZE_FEATURES) as *mut usize,
            fake_finalize_features as usize,
        );
    }

    // Allocate vdev buffer and populate the few fields probe reads.
    let vdev_buf = super::alloc::kzalloc(VDEV_BUF_SIZE, 0) as *mut u8;
    if vdev_buf.is_null() {
        log::warn!("kabi: failed to allocate fake virtio device buffer");
        return;
    }
    unsafe {
        core::ptr::write(
            vdev_buf.add(VDEV_OFF_CONFIG) as *mut usize,
            cfg_buf as usize,
        );
        core::ptr::write(
            vdev_buf.add(VDEV_OFF_FEATURES) as *mut u64,
            VIRTIO_F_VERSION_1,
        );
    }

    log::info!(
        "kabi: registered fake virtio device device_id={} vendor={:#x}, \
         vdev_buf={:#x}, cfg_ops={:#x}",
        VIRTIO_ID_INPUT, VIRTIO_ANY_ID,
        vdev_buf as usize, cfg_buf as usize,
    );

    let mut devices = FAKE_DEVICES.lock();
    devices.push(FakeVirtioDev {
        device_id: VIRTIO_ID_INPUT,
        vendor_id: VIRTIO_ANY_ID,
        vdev_buf,
    });
}

// ── Bus walking ──────────────────────────────────────────────

fn match_id_table(id_table: usize, device_id: u32, _vendor: u32) -> bool {
    if id_table == 0 {
        return false;
    }
    let mut idx: usize = 0;
    loop {
        let entry = id_table + idx * VIRTIO_DEVICE_ID_SIZE;
        let d = unsafe {
            core::ptr::read_unaligned(
                (entry + VIRTIO_DEVICE_ID_OFF_DEVICE) as *const u32,
            )
        };
        let _v = unsafe {
            core::ptr::read_unaligned(
                (entry + VIRTIO_DEVICE_ID_OFF_VENDOR) as *const u32,
            )
        };
        if d == 0 {
            return false;
        }
        if d == device_id {
            return true;
        }
        idx += 1;
        if idx > 32 {
            log::warn!("kabi: virtio id_table walk exceeded 32 entries");
            return false;
        }
    }
}

unsafe fn cstr_to_str(p: *const c_char) -> Option<&'static str> {
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

pub fn walk_and_probe() {
    ensure_fake_virtio_device();
    let drivers = REGISTERED.lock().clone();
    let devices: Vec<(u32, u32, *mut u8)> = FAKE_DEVICES
        .lock()
        .iter()
        .map(|d| (d.device_id, d.vendor_id, d.vdev_buf))
        .collect();

    log::info!(
        "kabi: virtio walk: {} driver(s), {} device(s)",
        drivers.len(),
        devices.len(),
    );

    for d in drivers.iter() {
        let id_table = unsafe {
            core::ptr::read_unaligned(
                (d.drv + VIRTIO_DRIVER_OFF_ID_TABLE) as *const usize,
            )
        };
        let probe_fn = unsafe {
            core::ptr::read_unaligned(
                (d.drv + VIRTIO_DRIVER_OFF_PROBE) as *const usize,
            )
        };
        let drv_name = unsafe { cstr_to_str(d.name_ptr) }.unwrap_or("?");

        if probe_fn == 0 {
            log::warn!(
                "kabi: virtio walk: driver '{}' has no probe; skipping",
                drv_name,
            );
            continue;
        }

        for &(device_id, vendor_id, vdev_buf) in devices.iter() {
            if match_id_table(id_table, device_id, vendor_id) {
                log::info!(
                    "kabi: virtio walk: probing driver '{}' against \
                     device_id={}",
                    drv_name, device_id,
                );
                let probe: extern "C" fn(*mut c_void) -> i32 =
                    unsafe { core::mem::transmute(probe_fn) };
                let rc = probe(vdev_buf as *mut c_void);
                log::info!(
                    "kabi: virtio walk: '{}' probe returned {}",
                    drv_name, rc,
                );
            }
        }
    }
}

// ── kABI entry points ────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn __register_virtio_driver(
    drv: *mut c_void,
    _owner: *const c_void,
) -> i32 {
    let drv_addr = drv as usize;
    // Driver name lives inside struct device_driver at offset 0
    // of struct virtio_driver (the embedded struct device_driver
    // starts with `const char *name`).
    let name_ptr = unsafe {
        core::ptr::read_unaligned(drv_addr as *const *const c_char)
    };
    log::info!(
        "kabi: __register_virtio_driver: name={:?}",
        unsafe { cstr_to_str(name_ptr) },
    );
    REGISTERED.lock().push(RegisteredVirtioDriver {
        drv: drv_addr,
        name_ptr,
    });
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn unregister_virtio_driver(_drv: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn virtio_reset_device(_dev: *mut c_void) {}

// virtqueue_*: ring-buffer helpers.  Probe-path stubs.

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
