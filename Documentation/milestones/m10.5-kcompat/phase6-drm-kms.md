# M10.5 Phase 6: DRM/KMS Display Stack

**Goal:** Load `amdgpu.ko` or `i915.ko` in display-only mode. A real monitor
shows the Kevlar framebuffer via the GPU's HDMI/DisplayPort output.

This phase targets **display output only** — no 3D acceleration. That comes
in Phase 7. The goal is modesetting: configure the display resolution, set a
framebuffer, see pixels on screen.

---

## DRM/KMS architecture

Linux's display stack (Direct Rendering Manager / Kernel Mode Setting):

```
Wayland compositor / X server
        │  ioctl (DRM_IOCTL_MODE_SETCRTC, DRM_IOCTL_PAGE_FLIP, ...)
        ▼
/dev/dri/card0   (DRM device node)
        │
        ▼
DRM core  (mode objects: CRTCs, planes, connectors, encoders)
        │
        ▼
amdgpu.ko / i915.ko  (hardware driver: scanout, display engine)
```

The userspace compositor talks to the DRM device via ioctls. The DRM core
routes to the hardware driver via callbacks.

---

## KMS object model

KMS (Kernel Mode Setting) uses abstract objects:

| Object | What it is | Example |
|--------|-----------|---------|
| **CRTC** | Display controller (scanout engine) | GPU display engine 0 |
| **Encoder** | Signal converter | TMDS encoder (HDMI signal) |
| **Connector** | Physical port | HDMI-A-1, DisplayPort-1 |
| **Plane** | Framebuffer layer | Primary plane (cursor plane) |
| **Framebuffer** | Buffer to scan out | GEM buffer object |

A display pipeline: Plane → CRTC → Encoder → Connector → Monitor

---

## DRM core functions

### Mode objects

| Function | Implementation |
|----------|----------------|
| `drm_dev_alloc(driver, dev)` | Allocate `struct drm_device` |
| `drm_dev_register(dev, flags)` | Register, create `/dev/dri/card0` |
| `drm_dev_unregister(dev)` | Unregister |
| `drm_crtc_init_with_planes(...)` | Initialize CRTC with primary+cursor plane |
| `drm_encoder_init(...)` | Initialize encoder |
| `drm_connector_init(...)` | Initialize connector |
| `drm_connector_attach_encoder(...)` | Wire connector to encoder |
| `drm_mode_config_init(dev)` | Initialize mode config |
| `drm_mode_config_reset(dev)` | Reset all objects to initial state |

### Mode setting (atomic)

Modern drivers use atomic modesetting:

```c
// Userspace submits a "state" describing the desired display configuration
// The driver validates it (drm_atomic_helper_check) then commits it
// (drm_atomic_helper_commit)
```

| Function | Implementation |
|----------|----------------|
| `drm_atomic_helper_check(dev, state)` | Validate proposed state |
| `drm_atomic_helper_commit(dev, state, nonblock)` | Apply state change |
| `drm_atomic_helper_wait_for_vblanks(dev, state)` | Wait for scanout |
| `drm_crtc_helper_set_mode(crtc, mode, ...)` | Legacy mode set |

### Framebuffer management

| Function | Implementation |
|----------|----------------|
| `drm_framebuffer_init(dev, fb, funcs)` | Register framebuffer |
| `drm_framebuffer_cleanup(fb)` | Unregister |
| `drm_gem_object_init(dev, obj, size)` | Initialize GEM object |
| `drm_gem_object_release(obj)` | Release GEM object |

---

## GEM (Graphics Execution Manager) — display only

For display-only (no 3D), we only need GEM for scanout buffers:
- Allocate a contiguous memory region (CMA — Contiguous Memory Allocator)
- Map it as a framebuffer
- Program the CRTC to scan out from its physical address

This avoids TTM (the full GPU memory manager) — TTM is only needed for
3D rendering workloads (Phase 7).

CMA-backed GEM objects (`drm_gem_cma_object`) are what simple display
drivers (e.g., `simpledrm`, `bochs`) use. This is the right starting point.

| Function | Implementation |
|----------|----------------|
| `drm_gem_cma_create(dev, size)` | Alloc CMA buffer (contiguous pages) |
| `drm_gem_cma_mmap(obj, vma)` | mmap to userspace |
| `dma_alloc_wc(dev, size, paddr, gfp)` | Write-combining DMA alloc |

---

## ioctls for display

The DRM device node must handle these ioctls:

| ioctl | Purpose |
|-------|---------|
| `DRM_IOCTL_VERSION` | Driver version info |
| `DRM_IOCTL_GET_CAP` | Query capabilities |
| `DRM_IOCTL_SET_CLIENT_CAP` | Declare compositor capabilities |
| `DRM_IOCTL_MODE_GETRESOURCES` | List CRTCs, encoders, connectors, FBs |
| `DRM_IOCTL_MODE_GETCONNECTOR` | Connector info + EDID modes |
| `DRM_IOCTL_MODE_GETCRTC` | CRTC state |
| `DRM_IOCTL_MODE_SETCRTC` | Set resolution + framebuffer (legacy) |
| `DRM_IOCTL_MODE_ADDFB` | Register framebuffer |
| `DRM_IOCTL_MODE_RMFB` | Unregister framebuffer |
| `DRM_IOCTL_MODE_DIRTYFB` | Flush damage region |
| `DRM_IOCTL_MODE_CREATE_DUMB` | Allocate dumb (linear) framebuffer |
| `DRM_IOCTL_MODE_MAP_DUMB` | Get mmap offset for dumb buffer |
| `DRM_IOCTL_MODE_DESTROY_DUMB` | Free dumb buffer |
| `DRM_IOCTL_GEM_CLOSE` | Release GEM handle |
| `DRM_IOCTL_PRIME_HANDLE_TO_FD` | DMA-BUF export (needed by Wayland) |
| `DRM_IOCTL_PRIME_FD_TO_HANDLE` | DMA-BUF import |
| `DRM_IOCTL_ATOMIC` | Atomic modesetting (required for Wayland) |
| `DRM_IOCTL_MODE_GETPROPERTY` | KMS property values |
| `DRM_IOCTL_MODE_SETPROPERTY` | Set KMS property |

The dumb buffer path (`CREATE_DUMB` + `MAP_DUMB` + `SETCRTC`) is the
simplest possible display path. X11 with fbdev driver uses this. Weston
uses atomic modesetting.

---

## amdgpu vs i915 for display-only

| Driver | Complexity | Target hardware |
|--------|-----------|-----------------|
| `amdgpu.ko` | High (AMDGPU is one driver for all AMD GPUs since GCN) | AMD RX 400 series+ |
| `i915.ko` | Very high (Intel uses separate code paths per gen) | Intel HD/Iris/Arc |
| `radeon.ko` | Medium (older AMD, simpler than amdgpu) | AMD HD 5000-R9 series |
| `nouveau.ko` | High (reverse-engineered NVIDIA) | NVIDIA pre-Turing |
| `simpledrm.ko` | Low (generic VESA/EFI framebuffer) | Any GPU with EFI GOP |

**Recommended path:** Start with `simpledrm.ko` (trivially simple — just
wraps the EFI GOP framebuffer into a DRM device). This validates the DRM
core, KMS object model, and ioctl dispatch before tackling amdgpu/i915.

`simpledrm` uses only ~500 of the ~800+ DRM symbols. Once it works, layer
in the GPU-specific drivers.

---

## EDID and display modes

The connector's available modes come from EDID (display's capability data):
- Fetched from the monitor via DDC (I²C over the display cable)
- Parsed into `struct drm_display_mode` (resolution, refresh rate, timings)
- Exposed via `DRM_IOCTL_MODE_GETCONNECTOR`

For Phase 6, a hardcoded 1920×1080@60Hz mode is acceptable. Real EDID
reading requires DDC/I²C access through the GPU — add in Phase 7.

---

## VBlank and page flip

Compositors use vblank interrupts to synchronize rendering. When the
current frame finishes scanning out, the CRTC generates a vblank interrupt:

```c
drm_crtc_handle_vblank(crtc);  // called from IRQ handler
```

`DRM_IOCTL_WAIT_VBLANK` blocks until the next vblank. Page flips atomically
swap the scanout buffer at vblank time, preventing tearing.

---

## Verification

### simpledrm + X11 fbdev

```bash
insmod simpledrm.ko
ls /dev/dri/  # card0 appears
# Start X with fbdev driver
X -config /etc/X11/xorg-fbdev.conf &
xterm &  # window appears on screen
```

### amdgpu display-only (no 3D)

```bash
insmod amdgpu.ko
ls /dev/dri/  # card0, renderD128
# weston --backend=drm → display output via KMS
weston &
# Wayland terminal appears on HDMI output
```

---

## Files to create/modify

- `kernel/kcompat/drm_core.rs` — DRM device, mode objects, ioctl dispatch
- `kernel/kcompat/drm_gem.rs` — GEM object management (CMA-backed)
- `kernel/kcompat/drm_kms.rs` — CRTC/plane/connector/encoder model
- `kernel/kcompat/drm_atomic.rs` — atomic modesetting helpers
- `kernel/kcompat/drm_vblank.rs` — vblank interrupt handling
- `kernel/kcompat/symbols_6_18.rs` — add DRM symbols (~300 for display)
- `kernel/device/dri.rs` — `/dev/dri/` device nodes
