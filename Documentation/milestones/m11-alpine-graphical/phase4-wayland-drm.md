# M11 Phase 4: Wayland + DRM/KMS

**Goal:** Modern display stack with DRM/KMS for Wayland compositors.

## Why DRM/KMS

X11 with fbdev works (Phase 3) but is legacy. Modern desktops (GNOME,
KDE, Sway) require Wayland which needs DRM/KMS for:
- Mode setting (resolution, refresh rate)
- Page flipping (vsync, double buffering)
- GBM buffer allocation
- Hardware cursor

## Scope

### DRM subsystem

`/dev/dri/card0` — the DRM device. Implements:

- `DRM_IOCTL_VERSION` — driver name/version
- `DRM_IOCTL_GET_CAP` — capabilities query
- `DRM_IOCTL_MODE_GETRESOURCES` — list CRTCs, connectors, encoders
- `DRM_IOCTL_MODE_GETCONNECTOR` — connector info (HDMI, VGA, virtual)
- `DRM_IOCTL_MODE_GETCRTC` / `SETCRTC` — configure display pipeline
- `DRM_IOCTL_MODE_GETFB` / `ADDFB` — framebuffer objects
- `DRM_IOCTL_MODE_PAGE_FLIP` — swap buffers (vsync)
- `DRM_IOCTL_MODE_CREATE_DUMB` / `MAP_DUMB` / `DESTROY_DUMB` — dumb buffer allocation

For QEMU's virtio-gpu or bochs-display, "dumb" buffers (CPU-rendered)
are sufficient. No 3D acceleration needed for XFCE/LXQt.

### GBM (Generic Buffer Management)

Mesa's GBM library uses DRM ioctls to allocate buffers. Weston and
other Wayland compositors use GBM. The kernel side is handled by the
DRM CREATE_DUMB / MAP_DUMB ioctls.

### SCM_RIGHTS (fd passing)

Wayland passes buffer fds between client and compositor via Unix socket
`sendmsg()`/`recvmsg()` with `SCM_RIGHTS` ancillary data. Need:
- `struct cmsghdr` parsing in recvmsg
- File descriptor transfer between processes
- Reference counting for shared fds

## Verification

```
apk add weston
weston --backend=drm-backend.so
# Weston terminal should appear in QEMU window
```
