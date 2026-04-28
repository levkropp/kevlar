# 270 — kABI K21: DRM ioctl dispatch returns kabi-drm 2.0.0

K21 lands.  Kevlar's `drm_ioctl()` is no longer a "return 0"
stub — it dispatches on the actual ioctl command code, reads
the userspace struct, fills in real values, and writes back.

```
kabi: DRM_IOCTL_VERSION returned rc=0
       name="kabi-drm" date="2026-04-27"
       desc="Kevlar kABI DRM driver"
       version=2.0.0
```

`make ARCH=arm64 test-module-k21` is the new regression target.
**22 kABI tests** pass.

A `drmGetVersion()` from libdrm-userspace would now succeed.
Mesa's first call after `drmOpen()` would succeed.  Xorg's
DRM-driver probe would, for the first time, get a sensible
response from Kevlar.

## What 90 lines of Rust do

The implementation is small.  drm_ioctl is one function; the
two handlers are five lines each.

```rust
const DRM_IOCTL_TYPE: u32 = b'd' as u32;       // 0x64
const DRM_IOCTL_NR_VERSION: u32 = 0x00;
const DRM_IOCTL_NR_GET_CAP: u32 = 0x0c;

#[unsafe(no_mangle)]
pub extern "C" fn drm_ioctl(_filp: *mut c_void, cmd: u32, arg: usize) -> isize {
    let nr = cmd & 0xff;
    let typ = (cmd >> 8) & 0xff;
    if typ != DRM_IOCTL_TYPE { return -25; }  // ENOTTY
    match nr {
        DRM_IOCTL_NR_VERSION => drm_ioctl_version(arg),
        DRM_IOCTL_NR_GET_CAP => drm_ioctl_get_cap(arg),
        _ => -25,
    }
}
```

Linux's `_IOC` macro encodes (dir, type, nr, size) into the
32-bit cmd value.  We mask out type and nr; the size field is
ignored because we know the struct layout — robust to any
size discrepancy from the userspace header.

For DRM_IOCTL_VERSION:

```rust
fn drm_ioctl_version(arg: usize) -> isize {
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
```

The `DrmVersion` struct mirrors Linux's `struct drm_version`
exactly (verified against `<drm/drm.h>` in Linux 7.0).  It's
`#[repr(C)]` Rust on one side, the same on Linux's side.  When
userspace marshals the struct and passes its address, the
bytes line up.

## How the dispatch trip works

When userspace runs `ioctl(open("/dev/dri/card0"), DRM_IOCTL_VERSION, &v)`:

```
sys_ioctl  [Kevlar VFS]
  → KabiCharDevFile::ioctl(cmd, arg)        [K4 adapter]
  → DRM_FOPS_ADAPTER.unlocked_ioctl(...)    [K20→K21 wired]
  → drm_ioctl_adapter(filp, cmd, arg)       [thin wrapper]
  → drm_ioctl(filp as *mut c_void, ...)     [K21 dispatcher]
  → drm_ioctl_version(arg)                  [K21 handler]
    → read DrmVersion from user
    → fill name/date/desc/version
    → write back
  → return 0
```

Six layers.  Each one has a well-defined boundary (Kevlar VFS
↔ K4 adapter ↔ K20 fops ↔ K21 dispatcher ↔ handler).  The
type signatures change at every boundary — `*mut FileShim` →
`*mut c_void` → `*mut DrmVersion` — but the underlying address
is the same byte the userspace ioctl(2) syscall handed the
kernel.

## The kernel-side smoke test pattern

K21 follows K4's `read_dev_for_test()` pattern: validate the
new dispatch path by calling it from the kernel itself, on
kernel-allocated buffers.  No userspace harness needed.

```rust
pub fn ioctl_smoke_test() {
    let cmd: u32 = 0xC000_0000
        | (DRM_IOCTL_TYPE << 8)
        | DRM_IOCTL_NR_VERSION
        | ((size_of::<DrmVersion>() as u32 & 0x3fff) << 16);

    let mut name_buf = [0u8; 64];
    /* ... */

    let mut v = DrmVersion {
        name: name_buf.as_mut_ptr(),
        name_len: name_buf.len(),
        /* ... */
    };
    let arg = &raw mut v as usize;

    let rc = drm_ioctl(core::ptr::null_mut(), cmd, arg);
    log::info!("DRM_IOCTL_VERSION returned rc={} name={:?}",
               rc, str::from_utf8(&name_buf[..v.name_len]).unwrap_or("?"));
}
```

`main.rs` calls this after `walk_and_probe()`.  The boot log
shows the round-trip works:

```
kabi: drm_dev_register: /dev/dri/card0 installed (major=226, minor=0)
kabi: PCI walk: 'cirrus-qemu' probe returned 0
kabi: DRM_IOCTL_VERSION returned rc=0 name="kabi-drm"
       date="2026-04-27" desc="Kevlar kABI DRM driver" version=2.0.0
```

Userspace test (K22) replaces the kernel-side smoke with a
real Alpine process opening `/dev/dri/card0` and ioctling it.

## Why version 2.0.0

Linux's DRM core declared driver-API stability at version 1.0
in 1999.  Most modern userspace expects to see version 2 (a
post-2008 modesetting-era convention).  We claim **kabi-drm
2.0.0** because:

- `2.x` signals "supports KMS modesetting" to libdrm.
- `kabi-drm` is the driver name; userspace tools that detect
  driver-by-name (xf86-video-* X drivers, Mesa's per-driver
  loaders) won't find a match for "kabi-drm" — and that's
  fine, they fall back to the generic modesetting path.
- The date `2026-04-27` is just K21's commit date; libdrm
  doesn't parse it.

When Mesa's first DRM_IOCTL_GET_CAP call follows
DRM_IOCTL_VERSION, it asks for capabilities like
`DRM_CAP_DUMB_BUFFER` or `DRM_CAP_PRIME`.  K21 returns 0 for
all of them.  Mesa interprets "no caps" as "you have a basic
DRM device" and proceeds.

## What's still ENOTTY

Every other DRM ioctl returns -25.  Notably:

- `DRM_IOCTL_MODE_GETRESOURCES` — returns the array of
  CRTCs/encoders/connectors.  Required for drmModeGetResources()
  to succeed.  K22+.
- `DRM_IOCTL_MODE_GETCRTC` / `_GETENCODER` / `_GETCONNECTOR`
- `DRM_IOCTL_MODE_ADDFB2` / `_RMFB`
- `DRM_IOCTL_MODE_PAGE_FLIP`
- `DRM_IOCTL_GEM_*` (close, flink, open, set_tiling, ...)
- `DRM_IOCTL_PRIME_HANDLE_TO_FD`

All of those need the drm_device's mode_config to be real.
K22+ wires that up.

## What didn't have to be done

- **Real `drm_compat_ioctl`.**  Still K17 stub returning 0.
  64-bit kernels handle 32-bit userspace through this; we
  only run 64-bit userspace on aarch64 right now.
- **copy_from_user / copy_to_user fault handling.**  K9's
  helpers do plain memcpy; no SIGSEGV trampoline.  Good
  enough for kernel-side smoke; K22+ when real userspace
  passes bogus pointers, we add fault recovery.
- **Per-driver private ioctls.**  cirrus has none.  bochs has
  none.  When a driver with custom ioctls arrives (i915 has
  many), we route via the driver's own fops.

## Cumulative kABI surface (K1-K21)

~331 exports.  ~50 shim modules.  Six DRM modules loadable.
**Two driver probes have run.  One userspace-visible
`/dev/dri/card0`.  DRM_IOCTL_VERSION returns real data.**

## Status

| Surface | Status |
|---|---|
| K1-K20 | ✅ |
| K21 — DRM ioctl dispatch (VERSION + GET_CAP) | ✅ |
| K22+ — userspace test + virtio_input probe | ⏳ |

## What K22 looks like

Three threads in priority order for "graphical ASAP":

1. **Real userspace test of /dev/dri/card0**.  An Alpine
   userspace program opens the char device and calls
   `ioctl(DRM_IOCTL_VERSION)`.  Validates the *full* syscall
   path, not just the kernel-side smoke test — Kevlar's
   sys_ioctl → VFS dispatch → K4 char device → DRM fops adapter
   → K21 dispatcher → DrmVersion-back-to-user.  This is the
   "we built it; does it work for real?" test.
2. **virtio bus walking + virtio_input probe**.  Same pattern
   as K19's PCI walking but for K12's virtio_input.  Probe
   fires, input device registers, `/dev/input/event0`
   becomes a real path.  Bridge for Xorg keyboard input.
3. **DRM_IOCTL_MODE_GETRESOURCES**.  Returns the array of
   CRTC/encoder/connector IDs.  Required for any modesetting
   userspace.  Bigger — needs the drm_device's mode_config
   to be real (currently a zero-filled buffer at K17).

The "graphical ASAP" arc is now **~3 milestones away**.  K21
was the proof that ioctl dispatch works; K22 is where it gets
tested by something that isn't us.
