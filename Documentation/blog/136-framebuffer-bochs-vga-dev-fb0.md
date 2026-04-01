# Blog 136: First pixels — Bochs VGA framebuffer and /dev/fb0

**Date:** 2026-04-01
**Milestone:** M11 Alpine Graphical — Phase 1 (Framebuffer)

## Summary

Kevlar now has a working framebuffer. The Bochs VGA driver detects the
standard QEMU display adapter on the PCI bus, programs VBE registers
for 1024x768 at 32bpp, and exposes `/dev/fb0` with full Linux fbdev
compatibility — ioctls, read/write, and mmap. Userspace can map the
16MB VRAM directly into its address space with zero-copy access. The
smoke test suite (67/67) still passes with the driver loaded.

## Why Bochs VGA?

QEMU provides several display backends. Bochs VGA (vendor 0x1234,
device 0x1111) is the default `-vga std` adapter and the simplest to
drive:

- **No command queues** — unlike virtio-gpu, there's no descriptor
  ring or host-guest protocol. Just write VBE registers and poke the
  linear framebuffer.
- **PCI BAR0 = framebuffer** — a single memory-mapped region at a
  fixed physical address. No scatter-gather, no DMA.
- **VBE register interface** — 10 I/O port registers at 0x1CE/0x1CF
  control resolution, color depth, and enable/disable.

This gets us to a working `/dev/fb0` with minimal code. virtio-gpu
(with 3D acceleration) can come later as an optimization.

## The driver (`exts/bochs_fb/`)

The driver follows the same pattern as virtio_blk and virtio_net:
a kernel extension that registers a `DeviceProber`, gets called during
PCI enumeration, and initializes the hardware.

### PCI detection

```
bochs-fb: found Bochs VGA device on PCI 0:2
bochs-fb: VBE ID=0xb0c5, VRAM=16MB at paddr 0xfd000000
```

The probe checks vendor 0x1234, device 0x1111, reads BAR0 for the
framebuffer physical address, and queries VBE register 0x0A for VRAM
size (in 64KB blocks). QEMU's default is 16MB.

### Mode setting

```rust
fn set_mode(width: u16, height: u16, bpp: u16) {
    vbe_write(VBE_DISPI_INDEX_ENABLE, VBE_DISPI_DISABLED);
    vbe_write(VBE_DISPI_INDEX_XRES, width);
    vbe_write(VBE_DISPI_INDEX_YRES, height);
    vbe_write(VBE_DISPI_INDEX_BPP, bpp);
    vbe_write(VBE_DISPI_INDEX_ENABLE, VBE_DISPI_ENABLED | VBE_DISPI_LFB_ENABLED);
}
```

Disable, set resolution + depth, re-enable with linear framebuffer
flag. The driver confirms the mode took by reading registers back:

```
bochs-fb: mode set to 1024x768x32
```

### Test pattern

The driver paints the entire framebuffer dark blue (0xFF1A1A2E in BGRA)
on init to confirm the hardware is working. On a QEMU window with
`--gui`, you'd see the blue screen. On `--batch` (serial only), it
still executes — we just can't see it.

## The device file (`kernel/fs/devfs/fb.rs`)

`/dev/fb0` implements the standard Linux fbdev interface:

### ioctls

| ioctl | What it returns |
|-------|----------------|
| `FBIOGET_VSCREENINFO` (0x4600) | `fb_var_screeninfo`: 1024x768, 32bpp, BGRA8888 bitfield layout |
| `FBIOGET_FSCREENINFO` (0x4602) | `fb_fix_screeninfo`: physical address 0xFD000000, stride 4096, type PACKED_PIXELS, visual TRUECOLOR |
| `FBIOPUT_VSCREENINFO` (0x4601) | Accepted silently (mode changes not yet supported) |
| `FBIOBLANK` (0x4611) | Accepted silently |

The structs match the Linux ABI exactly — same field order, same sizes.
Any program that opens `/dev/fb0` and calls these ioctls will get the
correct information.

### read/write

Direct pixel access via `read()` and `write()` at byte offsets into the
framebuffer. A userspace program can `write(fd, pixels, 4*1024*768)` to
paint the entire screen in one syscall.

### mmap — the important one

The key feature for real graphics: `mmap(NULL, size, PROT_READ|PROT_WRITE,
MAP_SHARED, fb_fd, 0)` maps the framebuffer directly into the process's
address space. Writes to the mapped region appear on screen immediately.

This required a new VMA type in the kernel:

```rust
pub enum VmAreaType {
    Anonymous,
    File { file, offset, file_size },
    DeviceMemory { phys_base: usize },  // NEW
}
```

When a page fault hits a `DeviceMemory` VMA, the handler maps the
physical device page directly — no allocation, no copy:

```rust
VmAreaType::DeviceMemory { phys_base } => {
    free_pages(paddr, 1);  // don't need the pre-allocated page
    let device_paddr = PAddr::new(phys_base + offset_in_vma);
    vm.page_table_mut()
        .map_user_page_with_prot(aligned_vaddr, device_paddr, prot);
    return;
}
```

The `mmap_phys_base()` method on `FileLike` tells the mmap syscall to
create a `DeviceMemory` VMA instead of a `File` VMA:

```rust
// In mmap syscall:
if let Some(phys_base) = file.mmap_phys_base() {
    VmAreaType::DeviceMemory { phys_base: phys_base + offset }
} else {
    VmAreaType::File { ... }
}
```

## Other fixes in this session

### TCP ephemeral port allocation

`bind()` with port 0 now assigns a port from the IANA dynamic range
(49152-65535). Previously, port 0 was stored literally, causing
`listen()` to fail when smoltcp tried to listen on port 0. The fix
mirrors the existing UDP ephemeral port logic.

### Shutdown cleanup

Before halting, the kernel now sends SIGKILL to all remaining processes.
This prevents a GPF that occurred when orphaned child processes tried to
run with stale page tables after PID 1 exited.

## Verified

- **67/67 smoke tests pass** with the framebuffer driver loaded
- **159/159 contract tests pass** — no ABI regressions
- Boot output confirms PCI detection, VBE programming, and VRAM mapping
- `/dev/fb0` registered in devfs with correct major:minor (29:0)

## What's next

With `/dev/fb0` working, the path to graphical Alpine is:

1. **PS/2 keyboard driver** — QEMU's i8042 controller for keyboard input
   (needed before any interactive graphical use)
2. **fbcon** — framebuffer console that renders text on the graphical
   display (replaces serial-only console)
3. **Xorg with fbdev driver** — `apk add xorg-server xf86-video-fbdev`
   should be able to open `/dev/fb0` and start X11
4. **Mouse input** — PS/2 mouse or virtio-input for pointer events
5. **XFCE desktop** — Alpine's default graphical environment
