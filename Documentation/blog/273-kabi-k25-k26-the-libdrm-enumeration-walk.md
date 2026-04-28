# 273 — kABI K25 + K26: the libdrm enumeration walk works

Two milestones, one post.  K25 implemented
DRM_IOCTL_MODE_GETRESOURCES; K26 implemented the three per-ID
follow-ups (GETCRTC, GETENCODER, GETCONNECTOR).  Together they
make `drmModeGetResources()` and the entire libdrm enumeration
walk succeed against Kevlar's `/dev/dri/card0`.

```
USERSPACE-DRM: name=kabi-drm version=2.0.0
USERSPACE-DRM: getres crtcs=1 connectors=1 encoders=1
               geom=320x200-4096x4096 crtc0=0x200 conn0=0x300 enc0=0x400
USERSPACE-DRM: getcrtc id=0x200 mode_valid=0
USERSPACE-DRM: getenc id=0x400 type=0 crtc=0x200
USERSPACE-DRM: getconn id=0x300 type=1 connection=1 enc=0x400
USERSPACE-DRM: done
```

`make ARCH=arm64 test-userspace-drm` is the new combined
regression target.  **27 kABI tests** pass.  Userspace believes
Kevlar has a 1-CRTC + 1-encoder + 1-VGA-connector display
topology, internally consistent, ready for modesetting.

## What libdrm wants

Every modesetting userspace — Xorg's modesetting driver,
kmscube, drm-info, weston-info — does the same dance after
opening `/dev/dri/card0`:

```c
int fd = open("/dev/dri/card0", O_RDWR);
ioctl(fd, DRM_IOCTL_VERSION, ...);          // K21
drmModeRes *res = drmModeGetResources(fd);   // K25
for (i = 0; i < res->count_crtcs; i++)
    drmModeGetCrtc(fd, res->crtcs[i]);       // K26
for (i = 0; i < res->count_connectors; i++)
    drmModeGetConnector(fd, res->connectors[i]);  // K26
for (i = 0; i < res->count_encoders; i++)
    drmModeGetEncoder(fd, res->encoders[i]);  // K26
```

That's libdrm's "discover the display topology" sequence.
Before K25, our `drm_ioctl` returned ENOTTY at the
GETRESOURCES step; userspace bailed.  After K26, the whole
walk finishes cleanly.

## K25: GETRESOURCES — the entry point

`MODE_GETRESOURCES` returns:

- Counts of how many CRTCs / connectors / encoders / fbs the
  device has.
- Arrays of their IDs (filled on a second call after
  userspace allocates room).
- The mode geometry bounds the device supports.

Standard libdrm pattern: call once with `count_*=0` to learn
the counts, allocate arrays, call again to fill them.  Our
implementation handles both:

```rust
fn drm_ioctl_mode_getresources(arg: usize) -> isize {
    let mut r = unsafe { core::ptr::read(arg as *const DrmModeCardRes) };

    let our_crtcs: u32 = 1;
    let our_connectors: u32 = 1;
    let our_encoders: u32 = 1;

    if r.crtc_id_ptr != 0 && r.count_crtcs > 0 {
        unsafe {
            core::ptr::write_unaligned(
                r.crtc_id_ptr as *mut u32,
                KABI_CRTC_ID_BASE,
            );
        }
    }
    // (same for connectors, encoders)

    r.count_crtcs = our_crtcs;
    r.count_connectors = our_connectors;
    r.count_encoders = our_encoders;
    r.min_width = 320;  r.max_width = 4096;
    r.min_height = 200; r.max_height = 4096;
    unsafe { core::ptr::write(arg as *mut DrmModeCardRes, r); }
    0
}
```

Synthesized IDs: CRTC=0x200, connector=0x300, encoder=0x400.
Picked to be visually distinct in logs; libdrm doesn't care
what numeric values we pick as long as they're stable.

## K26: the per-ID walk

After `drmModeGetResources()`, userspace calls
`drmModeGetCrtc(fd, crtc_id)` etc.  Each returns information
about one specific object.

The struct layouts are inherited verbatim from Linux's
`<drm/drm_mode.h>`:

```rust
#[repr(C)]
struct DrmModeCrtc {
    set_connectors_ptr: u64,
    count_connectors: u32,
    crtc_id: u32,
    fb_id: u32,
    x: u32, y: u32,
    gamma_size: u32,
    mode_valid: u32,
    mode: DrmModeModeinfo,    // 72 bytes embedded
}
// 108 bytes total
```

The handler validates the input ID, fills outputs.  Our
"current state" is:

- **CRTC 0x200**: no framebuffer attached (`fb_id=0`), no
  active mode (`mode_valid=0`), at origin, no gamma.
- **Encoder 0x400**: type DRM_MODE_ENCODER_NONE, drives CRTC
  0x200, `possible_crtcs=0x1` (bitmask: bit 0 = "the only
  CRTC").
- **Connector 0x300**: type DRM_MODE_CONNECTOR_VGA,
  **connection=connected**, attached encoder is 0x400.  No
  modes advertised yet (`count_modes=0`), no properties
  (`count_props=0`).

The "no modes advertised" piece is the next obstacle.  When
userspace tries to actually drive the display, it asks the
connector "what modes do you support?" via the same
GETCONNECTOR ioctl with `modes_ptr` non-null.  We currently
return zero modes.  Modesetting fails at the "no usable mode"
step.  K27+ adds at least one default mode (1024x768@60Hz).

But for *enumeration alone* — what tools like `drm-info` do —
K26 is enough.

## The internal-consistency property

What makes K26 interesting beyond "stub three ioctls" is that
the responses **internally cross-reference** correctly:

```
encoder 0x400:
    crtc_id=0x200             ← references CRTC 0x200
    possible_crtcs=0x1        ← bitmask: bit 0 = CRTC 0x200

connector 0x300:
    encoder_id=0x400          ← references encoder 0x400
    encoders[]=[0x400]        ← array of valid encoders
```

A userspace tool that walks the topology graph (encoder
→ its CRTC, connector → its encoder, etc.) gets a closed,
self-consistent answer.  No dangling references.  Xorg's
modesetting driver picks one of the resources and proceeds.

This consistency is what distinguishes "real-looking stub
data" from "obviously fake data."  When the modesetting
driver in Xorg sees `connector.encoder_id=0x400` and then
queries encoder 0x400 and gets `crtc_id=0x200` and queries
CRTC 0x200 and gets a valid response — it concludes there's
a working display pipeline.  Whether the pipeline drives a
real monitor is K28+ scope.

## What the structs look like

The four `#[repr(C)]` mirrors land in `kernel/kabi/drm_dev.rs`,
matching Linux's `<uapi/drm/drm_mode.h>`:

| Struct | Linux size (aarch64) | Kevlar size |
|---|---|---|
| `drm_mode_card_res` | 64 bytes | 64 bytes |
| `drm_mode_modeinfo` | 72 bytes | 72 bytes |
| `drm_mode_crtc` | 108 bytes | 108 bytes |
| `drm_mode_get_encoder` | 20 bytes | 20 bytes |
| `drm_mode_get_connector` | 84 bytes | 84 bytes |

All naturally aligned (u64 fields force 8-byte alignment;
embedded modeinfo aligns at u32 inside drm_mode_crtc).
Verified against Linux 7.0 sources before writing the
handlers.  Zero ABI surprises so far.

## What's still ENOTTY

Past K26, the following ioctls all still return -25:

- `DRM_IOCTL_MODE_ADDFB2` — create a framebuffer object from
  a buffer.
- `DRM_IOCTL_MODE_RMFB` — destroy one.
- `DRM_IOCTL_MODE_SETCRTC` — assign a framebuffer to a CRTC,
  set a mode, light up the display.
- `DRM_IOCTL_MODE_PAGE_FLIP` — async framebuffer swap.
- `DRM_IOCTL_GEM_*` — buffer object lifecycle.
- `DRM_IOCTL_PRIME_*` — buffer sharing across processes.
- `DRM_IOCTL_MODE_GETPROPERTY` and friends — connector
  properties (DPMS, EDID, etc.).
- `DRM_IOCTL_MODE_ATOMIC` — atomic modesetting.

Each is its own milestone.  K27+ picks the next one in the
order Xorg actually issues them.

## Cumulative kABI surface (K1-K26)

~331 exports.  ~50 shim modules.  Six DRM modules loadable.
Two probe-firing buses.  **Three ioctls past
DRM_IOCTL_VERSION on /dev/dri/card0 now succeed.**

## Status

| Surface | Status |
|---|---|
| K1-K24 | ✅ |
| K25 — DRM_IOCTL_MODE_GETRESOURCES | ✅ |
| K26 — DRM_IOCTL_MODE_GETCRTC + ENCODER + CONNECTOR | ✅ |
| K27+ — connector modes + framebuffer + SETCRTC | ⏳ |

## What K27 looks like

Two threads, in priority order for "graphical ASAP":

1. **Connector modes**: extend `drm_ioctl_mode_getconnector`
   to fill `modes_ptr` with at least one
   `drm_mode_modeinfo` entry — say, 1024x768@60Hz.  When
   userspace queries modes, it gets one.  Modesetting can
   pick it.
2. **Framebuffer creation + SETCRTC**: implement
   DRM_IOCTL_MODE_ADDFB2 (allocate a framebuffer slot, return
   an ID) and DRM_IOCTL_MODE_SETCRTC (attach the framebuffer
   to our CRTC, "set the mode").  Real Linux would now scan
   the framebuffer onto the screen; ours can't yet (no real
   GPU bringup) but a *successful* SETCRTC is meaningful — it
   means userspace believes it's driving the display.

(1) is small.  (2) is medium.  Together they get us to the
"userspace tries to display something and our DRM stack says
yes" milestone.  Pixels visible would be K28+.

The "graphical ASAP" arc is now ~3 milestones away.
