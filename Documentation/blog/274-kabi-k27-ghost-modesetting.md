# 274 — kABI K27: ghost modesetting

K27 lands.  Kevlar's `/dev/dri/card0` now accepts the full
userspace modesetting sequence — from `drmModeGetResources()`
through `drmModeAddFB2()` to `drmModeSetCrtc()` — and every
ioctl returns success.

```
USERSPACE-DRM: getconn.modes count=1 mode0=1024x768 1024x768@60Hz
USERSPACE-DRM: addfb2 fb_id=1
USERSPACE-DRM: setcrtc rc=0

kabi: MODE_ADDFB2: fb_id=1 1024x768 format=0x34325258
kabi: MODE_SETCRTC: crtc=0x200 fb=1 mode_valid=1 mode="1024x768"
```

`make ARCH=arm64 test-userspace-drm` is the new combined
regression target.  **28 kABI tests** pass.  Userspace
believes the display is configured.  Real Linux at this point
would be scanning a 1024x768 XRGB8888 framebuffer onto the
monitor.  Kevlar... isn't drawing pixels yet.  K27 is **ghost
modesetting** — every state transition succeeds, but the
shadow it casts on the screen doesn't render.

## What "ghost" means here

K17-K22 was about loading binaries and satisfying linker
references.  K23-K26 was about the *enumeration* surface —
walking the topology graph from CRTCs through encoders to
connectors.  K27 is the *commit* surface — userspace asks the
kernel to actually configure the hardware.

In real Linux, `MODE_SETCRTC` triggers:

1.  Validate the requested mode against the connector's EDID.
2.  Calculate timing dividers for the GPU's CRTC.
3.  Write to the GPU's MMIO registers to set up scanout.
4.  Allocate / activate display power planes.
5.  Sync the framebuffer pointer to the scanout engine.
6.  Wait for vblank.
7.  Return success.

In K27 Kevlar, `MODE_SETCRTC`:

1.  Validate `crtc_id == 0x200`.
2.  Record `(fb_id, mode_valid, mode, x, y)` in a `SpinLock`.
3.  Return success.

That's the difference between *configuring* hardware and
*recording that someone asked us to configure hardware*.  But
from the userspace perspective, both look identical: the
ioctl returned 0.  Every subsequent introspection ioctl
(`MODE_GETCRTC` reads back what was just set) returns
consistent data.  Xorg's modesetting driver would conclude
that the display is up and proceed to render.

## What's actually wired

Three new pieces, all in `kernel/kabi/drm_dev.rs`:

**A default mode the connector advertises**:

```rust
static DEFAULT_MODE: DrmModeModeinfo = DrmModeModeinfo {
    clock: 65000, hdisplay: 1024, hsync_start: 1048,
    hsync_end: 1184, htotal: 1344, /* ... */
    vdisplay: 768, vsync_start: 771, vsync_end: 777,
    vtotal: 806, vrefresh: 60,
    flags: 0x5,    // DRM_MODE_FLAG_PHSYNC | DRM_MODE_FLAG_PVSYNC
    type_: 0x48,   // DRM_MODE_TYPE_PREFERRED | DRM_MODE_TYPE_DRIVER
    name: *b"1024x768\0...",
};
```

VESA-standard 1024x768@60Hz timing.  When `MODE_GETCONNECTOR`
is called with `modes_ptr` non-null, this gets written to the
first slot.

**A framebuffer registry**:

```rust
static NEXT_FB_ID: AtomicU32 = AtomicU32::new(1);
static FBS: SpinLock<FbRegistry> = SpinLock::new(FbRegistry(Vec::new()));

fn drm_ioctl_mode_addfb2(arg: usize) -> isize {
    let mut cmd = read::<DrmModeFbCmd2>(arg);
    let id = NEXT_FB_ID.fetch_add(1, Ordering::Relaxed);
    FBS.lock().0.push(FbInfo { fb_id: id, width: cmd.width, ... });
    cmd.fb_id = id;
    write::<DrmModeFbCmd2>(arg, cmd);
    0
}
```

A counter, a vec of metadata, no actual buffer backing.
`ADDFB2` accepts any GEM handles — including 0 — without
validating.  Real Linux would have looked up the handle in
the calling process's GEM table and refused if it wasn't a
real allocated buffer object.  We just record the metadata
and hand back a fresh ID.

**A CRTC state slot**:

```rust
static CRTC_STATE: SpinLock<CrtcStateHolder> = ...;

fn drm_ioctl_mode_setcrtc(arg: usize) -> isize {
    let cmd = read::<DrmModeCrtc>(arg);
    if cmd.crtc_id != KABI_CRTC_ID_BASE { return EINVAL; }
    *CRTC_STATE.lock() = CrtcStateHolder(Some(CrtcState {
        fb_id: cmd.fb_id, mode_valid: cmd.mode_valid,
        mode: cmd.mode, x: cmd.x, y: cmd.y,
    }));
    0
}
```

One CRTC.  One state slot.  Whatever userspace last asked for
gets recorded.  K26's `MODE_GETCRTC` was updated to read from
this state, so a `SETCRTC → GETCRTC` round-trip returns the
just-set values.

That's it.  The whole milestone is ~140 lines of Rust.

## The XRGB8888 detail

When userspace calls `ADDFB2(width=1024, height=768,
pixel_format=0x34325258)`, the format code is the four-CC
`'X' 'R' '2' '4'` reversed (Linux little-endian convention).
That decodes to **DRM_FORMAT_XRGB8888** — the standard
32-bit-per-pixel, no-alpha format Xorg defaults to.

Our implementation doesn't care.  We record the format code
in the registry and hand back an ID.  When K28+ wires real
scanout, we'd use the format to compute byte stride and
configure the CRTC's pixel format register.  For now: it's
metadata.

The userspace test passed `0x34325258` because that's what
Xorg / kmscube would pass.  Our handler accepted it and moved
on.  At every layer, everyone is doing what they're supposed
to do.  No one has noticed yet that the framebuffer isn't real.

## Where the lie ends

If K27's userspace test then tried to:

- `mmap()` the fb_id to draw into it → fails (we don't have
  a backing buffer or mmap path).
- Read from a virtio-input event source → blocks forever.
- Call `MODE_PAGE_FLIP` to swap buffers → ENOTTY.
- Call any GEM ioctl → ENOTTY.

The walk and commit succeed.  The drawing doesn't.

That's the right trade-off for K27 because it lets us validate
the **decision-making layer** of modesetting — the part that
chooses a CRTC, picks a connector, allocates a framebuffer ID,
records state — without first having to solve the hard
problem of GPU memory mapping.  When K28+ adds real scanout,
the K27 state structures are exactly what feeds it.

## Internal-state consistency, again

K26 introduced internal-consistency: the topology cross-
references resolved.  K27 adds **temporal consistency**: state
written by one ioctl is observable from the next.

```c
/* Userspace: */
drmModeAddFB2(fd, 1024, 768, XRGB8888, ...);  /* fb_id=1 */
drmModeSetCrtc(fd, crtc_id=0x200, fb_id=1, mode);
drmModeCrtc *c = drmModeGetCrtc(fd, 0x200);
assert(c->fb_id == 1);          /* ← K27 makes this true */
assert(c->mode_valid == 1);     /* ← K27 makes this true */
strcmp(c->mode.name, "1024x768"); /* ← K27 makes this true */
```

That round-trip is what userspace tools verify after every
modeset.  It's also what makes the kernel's "I configured the
display" claim believable to userspace from the outside.
Without it, the next ioctl would surprise the userspace tool
and it would bail.

## Cumulative kABI surface (K1-K27)

~331 exports.  ~50 shim modules.  Six DRM modules loadable.
**Six DRM ioctls now succeed**: VERSION, GET_CAP,
MODE_GETRESOURCES, MODE_GETCRTC, MODE_GETENCODER,
MODE_GETCONNECTOR, MODE_ADDFB2, MODE_SETCRTC.  (That's eight.
The userspace handshake to "I have a configured display" is
complete.)

## Status

| Surface | Status |
|---|---|
| K1-K26 | ✅ |
| K27 — Connector modes + ADDFB2 + SETCRTC | ✅ |
| K28+ — real pixel scanout OR fbcon OR keystrokes | ⏳ |

## What K28 looks like

Three roads, in roughly equal-priority order toward graphical
Alpine:

1.  **Real pixel scanout**.  The recorded `(fb_id, mode)` from
    SETCRTC needs to drive cirrus's MMIO BAR (currently a 4KB
    zero buffer).  Implement at least the cirrus VGA register
    path so the requested resolution + a fake VRAM region
    actually map onto QEMU's emulated display.  When this
    works, **pixels appear in the QEMU window**.  Big.
2.  **fbcon binding to /dev/dri/card0**.  Linux's fbcon hooks
    into a DRM device's modeset and writes kernel printk's to
    the framebuffer.  Smaller than full Xorg pipeline; first
    "kernel console visible" milestone.
3.  **Real virtqueue event flow** → keystrokes at
    /dev/input/event2.  Pivots to input.  Useful but doesn't
    move the display path forward.

The "graphical ASAP" arc is now **~2 milestones** away.  K27
was the last "stub more ioctls" milestone before the real
work — getting bytes from a recorded framebuffer onto an
actual display surface — begins.
