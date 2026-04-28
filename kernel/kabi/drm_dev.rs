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

extern "C" fn drm_ioctl_adapter(
    filp: *mut crate::kabi::fops::FileShim,
    cmd: u32,
    arg: usize,
) -> isize {
    drm_ioctl(filp as *mut c_void, cmd, arg)
}

/// Static FileOperationsShim used by every /dev/dri/cardN we
/// install.  Routes K4 char-device callbacks to the K17/K21 drm_*
/// stubs (drm_open / _release / _read / _poll / _ioctl).
struct DrmFopsHolder(crate::kabi::fops::FileOperationsShim);
unsafe impl Sync for DrmFopsHolder {}

static DRM_FOPS_ADAPTER: DrmFopsHolder = DrmFopsHolder(
    crate::kabi::fops::FileOperationsShim {
        owner: core::ptr::null(),
        llseek: None,
        read: Some(drm_read_adapter),
        write: None,
        unlocked_ioctl: Some(drm_ioctl_adapter),
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

// ── DRM ioctl dispatch (K21) ──────────────────────────────────

const DRM_IOCTL_TYPE: u32 = b'd' as u32; // 0x64
const DRM_IOCTL_NR_VERSION: u32 = 0x00;
const DRM_IOCTL_NR_GET_CAP: u32 = 0x0c;

const ENOTTY: isize = -25;
const EFAULT: isize = -14;

#[repr(C)]
pub struct DrmVersion {
    pub version_major: i32,
    pub version_minor: i32,
    pub version_patchlevel: i32,
    pub _pad: i32,
    pub name_len: usize,
    pub name: *mut u8,
    pub date_len: usize,
    pub date: *mut u8,
    pub desc_len: usize,
    pub desc: *mut u8,
}

#[repr(C)]
struct DrmGetCap {
    capability: u64,
    value: u64,
}

fn copy_to_user_truncate(src: &[u8], dst: *mut u8, len: &mut usize) {
    if !dst.is_null() && *len > 0 {
        let n = src.len().min(*len);
        unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), dst, n); }
    }
    *len = src.len();
}

fn drm_ioctl_version(arg: usize) -> isize {
    if arg == 0 {
        return EFAULT;
    }
    let mut v = unsafe { core::ptr::read(arg as *const DrmVersion) };

    v.version_major = 2;
    v.version_minor = 0;
    v.version_patchlevel = 0;

    static NAME: &[u8] = b"kabi-drm";
    static DATE: &[u8] = b"2026-04-27";
    static DESC: &[u8] = b"Kevlar kABI DRM driver";

    copy_to_user_truncate(NAME, v.name, &mut v.name_len);
    copy_to_user_truncate(DATE, v.date, &mut v.date_len);
    copy_to_user_truncate(DESC, v.desc, &mut v.desc_len);

    unsafe { core::ptr::write(arg as *mut DrmVersion, v); }
    0
}

fn drm_ioctl_get_cap(arg: usize) -> isize {
    if arg == 0 {
        return EFAULT;
    }
    let mut c = unsafe { core::ptr::read(arg as *const DrmGetCap) };
    c.value = 0;
    unsafe { core::ptr::write(arg as *mut DrmGetCap, c); }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn drm_ioctl(
    _filp: *mut c_void,
    cmd: u32,
    arg: usize,
) -> isize {
    let nr = cmd & 0xff;
    let typ = (cmd >> 8) & 0xff;
    if typ != DRM_IOCTL_TYPE {
        return ENOTTY;
    }
    match nr {
        DRM_IOCTL_NR_VERSION => drm_ioctl_version(arg),
        DRM_IOCTL_NR_GET_CAP => drm_ioctl_get_cap(arg),
        _ => ENOTTY,
    }
}

/// Kernel-side smoke test: issue DRM_IOCTL_VERSION against the
/// dispatcher and log the result.  Verifies the K21 path
/// end-to-end without needing a userspace program.  Called from
/// `kernel/main.rs` after `walk_and_probe()`.
pub fn ioctl_smoke_test() {
    let cmd: u32 = 0xC000_0000
        | (DRM_IOCTL_TYPE << 8)
        | DRM_IOCTL_NR_VERSION
        | ((core::mem::size_of::<DrmVersion>() as u32 & 0x3fff) << 16);

    let mut name_buf = [0u8; 64];
    let mut date_buf = [0u8; 64];
    let mut desc_buf = [0u8; 64];

    let mut v = DrmVersion {
        version_major: 0,
        version_minor: 0,
        version_patchlevel: 0,
        _pad: 0,
        name_len: name_buf.len(),
        name: name_buf.as_mut_ptr(),
        date_len: date_buf.len(),
        date: date_buf.as_mut_ptr(),
        desc_len: desc_buf.len(),
        desc: desc_buf.as_mut_ptr(),
    };

    let arg = &raw mut v as usize;
    let rc = drm_ioctl(core::ptr::null_mut(), cmd, arg);

    let nb = v.name_len.min(name_buf.len());
    let db = v.date_len.min(date_buf.len());
    let cb = v.desc_len.min(desc_buf.len());
    log::info!(
        "kabi: DRM_IOCTL_VERSION returned rc={} name={:?} date={:?} \
         desc={:?} version={}.{}.{}",
        rc,
        core::str::from_utf8(&name_buf[..nb]).unwrap_or("?"),
        core::str::from_utf8(&date_buf[..db]).unwrap_or("?"),
        core::str::from_utf8(&desc_buf[..cb]).unwrap_or("?"),
        v.version_major,
        v.version_minor,
        v.version_patchlevel,
    );
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
