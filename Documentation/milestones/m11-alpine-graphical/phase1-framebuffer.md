# M11 Phase 1: Framebuffer

**Goal:** Memory-mapped framebuffer at `/dev/fb0` with pixels on screen.

## Approach: Bochs VGA (simplest) or virtio-gpu

**Bochs VGA** is QEMU's default VGA device. It exposes a PCI BAR with a
linear framebuffer. No GPU commands, no 3D — just write pixels to memory
and they appear on screen. This is the simplest path to a working display.

QEMU flags: `-device VGA` (default) or `-device bochs-display`

**virtio-gpu** is more complex but supports hardware-accelerated 2D/3D
(via virgl). It requires a virtio transport layer and a DRM driver.
This is the long-term target but not needed for Phase 1.

## Kernel Changes

### PCI device discovery

Bochs VGA is a PCI device. We need:
- PCI config space read (BAR0 = framebuffer physical address)
- Map the framebuffer BAR into kernel address space
- Our existing PCI scanning code (for virtio) can be extended

### Framebuffer subsystem

- `/dev/fb0` device node
- `ioctl(FBIOGET_VSCREENINFO)` — returns resolution, bpp, stride
- `ioctl(FBIOGET_FSCREENINFO)` — returns physical address, line length
- `mmap()` — maps the framebuffer into userspace
- `write()` — optional, for direct pixel writes

### Resolution configuration

Bochs VGA: write to VBE dispi registers (I/O ports 0x1CE/0x1CF) to set
resolution. Default 1024x768x32 is fine for XFCE.

## Verification

```
# Boot Alpine, install fbset, check framebuffer
apk add fbset
fbset -i  # should show resolution
# Write test pattern: dd if=/dev/urandom of=/dev/fb0 bs=4096
```

Success: QEMU window shows colored pixels from `/dev/fb0` writes.
