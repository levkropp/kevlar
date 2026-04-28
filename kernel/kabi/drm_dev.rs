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
    size: usize,
    offset: usize,
) -> *mut c_void {
    // K19: real allocation — caller's wrapping struct embeds a
    // `struct drm_device` at `offset` within a `size`-byte
    // allocation.  Real Linux uses drm_managed so the allocation
    // is freed when the device is destroyed; ours leaks.
    if size == 0 {
        return core::ptr::null_mut();
    }
    let buf = super::alloc::kzalloc(size, 0) as *mut u8;
    if buf.is_null() {
        return core::ptr::null_mut();
    }
    log::info!(
        "kabi: __devm_drm_dev_alloc: size={} offset={} buf={:#x} drm_dev={:#x}",
        size,
        offset,
        buf as usize,
        unsafe { buf.add(offset) } as usize,
    );
    unsafe { buf.add(offset) as *mut c_void }
}

// ── DRM minor allocation + char-device registration ───────────

use core::sync::atomic::{AtomicU32, Ordering};

/// Counter for /dev/dri/cardN names.  Linux uses major=226 for DRM
/// primary nodes; we follow that convention so userspace tools
/// recognize the major.
static NEXT_DRM_MINOR: AtomicU32 = AtomicU32::new(0);
const DRM_MAJOR: u32 = 226;

/// Adapter fops: forward Kevlar K4 char-device callbacks to the
/// drm_open / drm_release / drm_read / drm_poll / drm_ioctl /
/// drm_compat_ioctl stubs.  All return 0 / 0-bytes today; real
/// dispatch lands K21+.
extern "C" fn drm_open_adapter(
    inode: *mut crate::kabi::fops::InodeShim,
    filp: *mut crate::kabi::fops::FileShim,
) -> i32 {
    drm_open(inode as *mut c_void, filp as *mut c_void)
}

extern "C" fn drm_release_adapter(
    inode: *mut crate::kabi::fops::InodeShim,
    filp: *mut crate::kabi::fops::FileShim,
) -> i32 {
    drm_release(inode as *mut c_void, filp as *mut c_void)
}

extern "C" fn drm_read_adapter(
    filp: *mut crate::kabi::fops::FileShim,
    buf: *mut u8,
    count: usize,
    ppos: *mut i64,
) -> isize {
    drm_read(filp as *mut c_void, buf as *mut c_void, count, ppos as *mut c_void)
}

extern "C" fn drm_poll_adapter(
    filp: *mut crate::kabi::fops::FileShim,
    wait: *const c_void,
) -> u32 {
    drm_poll(filp as *mut c_void, wait as *mut c_void)
}

/// Static FileOperationsShim used by every /dev/dri/cardN we
/// install.  All slots route to the shared K17 drm_* stubs.
struct DrmFopsHolder(crate::kabi::fops::FileOperationsShim);
unsafe impl Sync for DrmFopsHolder {}

static DRM_FOPS_ADAPTER: DrmFopsHolder = DrmFopsHolder(
    crate::kabi::fops::FileOperationsShim {
        owner: core::ptr::null(),
        llseek: None,
        read: Some(drm_read_adapter),
        write: None,
        unlocked_ioctl: None,
        poll: Some(drm_poll_adapter),
        mmap: None,
        open: Some(drm_open_adapter),
        release: Some(drm_release_adapter),
    },
);

#[unsafe(no_mangle)]
pub extern "C" fn drm_dev_register(_dev: *mut c_void, _flags: u64) -> i32 {
    let minor = NEXT_DRM_MINOR.fetch_add(1, Ordering::Relaxed);
    let card_name = alloc::format!("card{}", minor);
    crate::kabi::cdev::install_chrdev_in_subdir(
        DRM_MAJOR,
        minor,
        1,
        "dri",
        &card_name,
        &DRM_FOPS_ADAPTER.0,
    );
    log::info!(
        "kabi: drm_dev_register: /dev/dri/{} installed (major={}, minor={})",
        card_name, DRM_MAJOR, minor
    );
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
