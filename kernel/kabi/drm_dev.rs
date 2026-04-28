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

// ── DRM ioctl dispatch (K21 + K25) ─────────────────────────────

const DRM_IOCTL_TYPE: u32 = b'd' as u32; // 0x64
const DRM_IOCTL_NR_VERSION: u32 = 0x00;
const DRM_IOCTL_NR_GET_CAP: u32 = 0x0c;
const DRM_IOCTL_NR_MODE_GETRESOURCES: u32 = 0xA0;
const DRM_IOCTL_NR_MODE_GETCRTC: u32 = 0xA1;
const DRM_IOCTL_NR_MODE_SETCRTC: u32 = 0xA2;
const DRM_IOCTL_NR_MODE_GETENCODER: u32 = 0xA6;
const DRM_IOCTL_NR_MODE_GETCONNECTOR: u32 = 0xA7;
const DRM_IOCTL_NR_MODE_ADDFB2: u32 = 0xB8;

// Synthesized object IDs.  drmModeGet{Crtc,Connector,Encoder}
// from libdrm walks these in K26+.
const KABI_CRTC_ID_BASE: u32 = 0x0200;
const KABI_CONNECTOR_ID_BASE: u32 = 0x0300;
const KABI_ENCODER_ID_BASE: u32 = 0x0400;

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

#[repr(C)]
pub struct DrmModeCardRes {
    pub fb_id_ptr: u64,
    pub crtc_id_ptr: u64,
    pub connector_id_ptr: u64,
    pub encoder_id_ptr: u64,
    pub count_fbs: u32,
    pub count_crtcs: u32,
    pub count_connectors: u32,
    pub count_encoders: u32,
    pub min_width: u32,
    pub max_width: u32,
    pub min_height: u32,
    pub max_height: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct DrmModeModeinfo {
    clock: u32,
    hdisplay: u16,
    hsync_start: u16,
    hsync_end: u16,
    htotal: u16,
    hskew: u16,
    vdisplay: u16,
    vsync_start: u16,
    vsync_end: u16,
    vtotal: u16,
    vscan: u16,
    vrefresh: u32,
    flags: u32,
    type_: u32,
    name: [u8; 32],
}

#[repr(C)]
struct DrmModeCrtc {
    set_connectors_ptr: u64,
    count_connectors: u32,
    crtc_id: u32,
    fb_id: u32,
    x: u32,
    y: u32,
    gamma_size: u32,
    mode_valid: u32,
    mode: DrmModeModeinfo,
}

#[repr(C)]
struct DrmModeGetEncoder {
    encoder_id: u32,
    encoder_type: u32,
    crtc_id: u32,
    possible_crtcs: u32,
    possible_clones: u32,
}

#[repr(C)]
struct DrmModeGetConnector {
    encoders_ptr: u64,
    modes_ptr: u64,
    props_ptr: u64,
    prop_values_ptr: u64,
    count_modes: u32,
    count_props: u32,
    count_encoders: u32,
    encoder_id: u32,
    connector_id: u32,
    connector_type: u32,
    connector_type_id: u32,
    connection: u32,
    mm_width: u32,
    mm_height: u32,
    subpixel: u32,
    pad: u32,
}

#[repr(C)]
struct DrmModeFbCmd2 {
    fb_id: u32,
    width: u32,
    height: u32,
    pixel_format: u32,
    flags: u32,
    handles: [u32; 4],
    pitches: [u32; 4],
    offsets: [u32; 4],
    modifier: [u64; 4],
}

// VESA 1024x768@60Hz timing.  Advertised by GETCONNECTOR when
// userspace asks for modes.
static DEFAULT_MODE: DrmModeModeinfo = DrmModeModeinfo {
    clock: 65000,
    hdisplay: 1024,
    hsync_start: 1048,
    hsync_end: 1184,
    htotal: 1344,
    hskew: 0,
    vdisplay: 768,
    vsync_start: 771,
    vsync_end: 777,
    vtotal: 806,
    vscan: 0,
    vrefresh: 60,
    flags: 0x5,   // DRM_MODE_FLAG_PHSYNC | DRM_MODE_FLAG_PVSYNC
    type_: 0x48,  // DRM_MODE_TYPE_PREFERRED | DRM_MODE_TYPE_DRIVER
    name: *b"1024x768\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
};

// ── Framebuffer registry (K27 ADDFB2) ─────────────────────────

#[derive(Clone, Copy)]
struct FbInfo {
    fb_id: u32,
    width: u32,
    height: u32,
    pixel_format: u32,
}

struct FbRegistry(alloc::vec::Vec<FbInfo>);
unsafe impl Send for FbRegistry {}

static NEXT_FB_ID: AtomicU32 = AtomicU32::new(1);
static FBS: kevlar_platform::spinlock::SpinLock<FbRegistry> =
    kevlar_platform::spinlock::SpinLock::new(FbRegistry(alloc::vec::Vec::new()));

// ── CRTC current state (K27 SETCRTC) ──────────────────────────

#[derive(Clone, Copy)]
struct CrtcState {
    fb_id: u32,
    mode_valid: u32,
    mode: DrmModeModeinfo,
    x: u32,
    y: u32,
}

struct CrtcStateHolder(Option<CrtcState>);
unsafe impl Send for CrtcStateHolder {}

static CRTC_STATE: kevlar_platform::spinlock::SpinLock<CrtcStateHolder> =
    kevlar_platform::spinlock::SpinLock::new(CrtcStateHolder(None));

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

fn drm_ioctl_mode_getresources(arg: usize) -> isize {
    if arg == 0 {
        return EFAULT;
    }
    let mut r = unsafe { core::ptr::read(arg as *const DrmModeCardRes) };

    // Our advertised counts: 1 CRTC + 1 connector + 1 encoder + 0 fbs.
    let our_fbs: u32 = 0;
    let our_crtcs: u32 = 1;
    let our_connectors: u32 = 1;
    let our_encoders: u32 = 1;

    // Linux semantics: fill the user-supplied ID arrays up to
    // min(in_count, our_count).  No FBs to write.

    if r.crtc_id_ptr != 0 && r.count_crtcs > 0 {
        let n = r.count_crtcs.min(our_crtcs);
        for i in 0..n {
            unsafe {
                core::ptr::write_unaligned(
                    (r.crtc_id_ptr as *mut u32).add(i as usize),
                    KABI_CRTC_ID_BASE + i,
                );
            }
        }
    }
    if r.connector_id_ptr != 0 && r.count_connectors > 0 {
        let n = r.count_connectors.min(our_connectors);
        for i in 0..n {
            unsafe {
                core::ptr::write_unaligned(
                    (r.connector_id_ptr as *mut u32).add(i as usize),
                    KABI_CONNECTOR_ID_BASE + i,
                );
            }
        }
    }
    if r.encoder_id_ptr != 0 && r.count_encoders > 0 {
        let n = r.count_encoders.min(our_encoders);
        for i in 0..n {
            unsafe {
                core::ptr::write_unaligned(
                    (r.encoder_id_ptr as *mut u32).add(i as usize),
                    KABI_ENCODER_ID_BASE + i,
                );
            }
        }
    }

    // Always write our actual counts back.
    r.count_fbs = our_fbs;
    r.count_crtcs = our_crtcs;
    r.count_connectors = our_connectors;
    r.count_encoders = our_encoders;

    // Permissive geometry — matches Linux's modesetting drivers
    // for emulated VGA hardware.
    r.min_width = 320;
    r.max_width = 4096;
    r.min_height = 200;
    r.max_height = 4096;

    unsafe { core::ptr::write(arg as *mut DrmModeCardRes, r); }
    0
}

const EINVAL: isize = -22;

fn drm_ioctl_mode_getcrtc(arg: usize) -> isize {
    if arg == 0 {
        return EFAULT;
    }
    let mut c = unsafe { core::ptr::read(arg as *const DrmModeCrtc) };
    if c.crtc_id != KABI_CRTC_ID_BASE {
        return EINVAL;
    }
    let state = CRTC_STATE.lock().0;
    if let Some(s) = state {
        c.fb_id = s.fb_id;
        c.x = s.x;
        c.y = s.y;
        c.mode_valid = s.mode_valid;
        c.mode = s.mode;
    } else {
        c.fb_id = 0;
        c.x = 0;
        c.y = 0;
        c.mode_valid = 0;
        c.mode = unsafe { core::mem::zeroed() };
    }
    c.gamma_size = 0;
    unsafe { core::ptr::write(arg as *mut DrmModeCrtc, c); }
    0
}

fn drm_ioctl_mode_getencoder(arg: usize) -> isize {
    if arg == 0 {
        return EFAULT;
    }
    let mut e = unsafe { core::ptr::read(arg as *const DrmModeGetEncoder) };
    if e.encoder_id != KABI_ENCODER_ID_BASE {
        return EINVAL;
    }
    e.encoder_type = 0; // DRM_MODE_ENCODER_NONE
    e.crtc_id = KABI_CRTC_ID_BASE;
    e.possible_crtcs = 0x1; // bit 0 = our one CRTC
    e.possible_clones = 0;
    unsafe { core::ptr::write(arg as *mut DrmModeGetEncoder, e); }
    0
}

fn drm_ioctl_mode_getconnector(arg: usize) -> isize {
    if arg == 0 {
        return EFAULT;
    }
    let mut c = unsafe { core::ptr::read(arg as *const DrmModeGetConnector) };
    if c.connector_id != KABI_CONNECTOR_ID_BASE {
        return EINVAL;
    }
    // Fill the encoders array if userspace allocated room.
    if c.encoders_ptr != 0 && c.count_encoders > 0 {
        unsafe {
            core::ptr::write_unaligned(
                c.encoders_ptr as *mut u32,
                KABI_ENCODER_ID_BASE,
            );
        }
    }
    // K27: advertise one default mode (1024x768@60Hz) when
    // userspace allocates room.
    if c.modes_ptr != 0 && c.count_modes > 0 {
        unsafe {
            core::ptr::write_unaligned(
                c.modes_ptr as *mut DrmModeModeinfo,
                DEFAULT_MODE,
            );
        }
    }
    c.count_modes = 1;
    c.count_props = 0;
    c.count_encoders = 1;
    c.encoder_id = KABI_ENCODER_ID_BASE;
    c.connector_type = 1; // DRM_MODE_CONNECTOR_VGA
    c.connector_type_id = 0;
    c.connection = 1; // connector_status_connected
    c.mm_width = 0;
    c.mm_height = 0;
    c.subpixel = 0;
    c.pad = 0;
    unsafe { core::ptr::write(arg as *mut DrmModeGetConnector, c); }
    0
}

fn drm_ioctl_mode_addfb2(arg: usize) -> isize {
    if arg == 0 {
        return EFAULT;
    }
    let mut cmd = unsafe { core::ptr::read(arg as *const DrmModeFbCmd2) };
    let id = NEXT_FB_ID.fetch_add(1, Ordering::Relaxed);
    let info = FbInfo {
        fb_id: id,
        width: cmd.width,
        height: cmd.height,
        pixel_format: cmd.pixel_format,
    };
    FBS.lock().0.push(info);
    cmd.fb_id = id;
    log::info!(
        "kabi: MODE_ADDFB2: fb_id={} {}x{} format={:#x}",
        info.fb_id, info.width, info.height, info.pixel_format,
    );
    unsafe { core::ptr::write(arg as *mut DrmModeFbCmd2, cmd); }
    0
}

fn drm_ioctl_mode_setcrtc(arg: usize) -> isize {
    if arg == 0 {
        return EFAULT;
    }
    let cmd = unsafe { core::ptr::read(arg as *const DrmModeCrtc) };
    if cmd.crtc_id != KABI_CRTC_ID_BASE {
        return EINVAL;
    }
    *CRTC_STATE.lock() = CrtcStateHolder(Some(CrtcState {
        fb_id: cmd.fb_id,
        mode_valid: cmd.mode_valid,
        mode: cmd.mode,
        x: cmd.x,
        y: cmd.y,
    }));
    let mode_name_str = core::str::from_utf8(&cmd.mode.name)
        .unwrap_or("?")
        .trim_end_matches('\0');
    log::info!(
        "kabi: MODE_SETCRTC: crtc=0x{:x} fb={} mode_valid={} mode={:?}",
        cmd.crtc_id, cmd.fb_id, cmd.mode_valid, mode_name_str,
    );
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
        DRM_IOCTL_NR_MODE_GETRESOURCES => drm_ioctl_mode_getresources(arg),
        DRM_IOCTL_NR_MODE_GETCRTC => drm_ioctl_mode_getcrtc(arg),
        DRM_IOCTL_NR_MODE_SETCRTC => drm_ioctl_mode_setcrtc(arg),
        DRM_IOCTL_NR_MODE_GETENCODER => drm_ioctl_mode_getencoder(arg),
        DRM_IOCTL_NR_MODE_GETCONNECTOR => drm_ioctl_mode_getconnector(arg),
        DRM_IOCTL_NR_MODE_ADDFB2 => drm_ioctl_mode_addfb2(arg),
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

    // K25: also exercise DRM_IOCTL_MODE_GETRESOURCES.
    let res_cmd: u32 = 0xC000_0000
        | (DRM_IOCTL_TYPE << 8)
        | DRM_IOCTL_NR_MODE_GETRESOURCES
        | ((core::mem::size_of::<DrmModeCardRes>() as u32 & 0x3fff) << 16);

    let mut crtc_ids = [0u32; 4];
    let mut conn_ids = [0u32; 4];
    let mut enc_ids = [0u32; 4];
    let mut res = DrmModeCardRes {
        fb_id_ptr: 0,
        crtc_id_ptr: crtc_ids.as_mut_ptr() as u64,
        connector_id_ptr: conn_ids.as_mut_ptr() as u64,
        encoder_id_ptr: enc_ids.as_mut_ptr() as u64,
        count_fbs: 0,
        count_crtcs: 4,
        count_connectors: 4,
        count_encoders: 4,
        min_width: 0,
        max_width: 0,
        min_height: 0,
        max_height: 0,
    };
    let res_arg = &raw mut res as usize;
    let res_rc = drm_ioctl(core::ptr::null_mut(), res_cmd, res_arg);
    log::info!(
        "kabi: DRM_IOCTL_MODE_GETRESOURCES rc={} crtcs={} crtc[0]={:#x} \
         connectors={} encoders={} fbs={} geom={}x{}-{}x{}",
        res_rc,
        res.count_crtcs, crtc_ids[0],
        res.count_connectors, res.count_encoders, res.count_fbs,
        res.min_width, res.min_height, res.max_width, res.max_height,
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
