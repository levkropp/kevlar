# 276 — kABI K29: pixels

K29 lands.  29 milestones in, 29 milestones since "load a
hello-world `.ko`."  This one ships a screenshot.

![K29 red square](k29-red-square.png)

That square is 100×100 pixels of RGB `(255, 0, 0)` at offset
`(10, 10)` on a 1024×768 framebuffer.  An Alpine userspace
binary running as Kevlar's PID 1 painted it through the full
DRM ioctl path:

```
fd = open("/dev/dri/card0", O_RDWR);
ioctl(fd, DRM_IOCTL_VERSION, ...);              // K21
ioctl(fd, DRM_IOCTL_MODE_GETRESOURCES, ...);    // K25
ioctl(fd, DRM_IOCTL_MODE_GETCRTC, ...);         // K26
ioctl(fd, DRM_IOCTL_MODE_GETENCODER, ...);      // K26
ioctl(fd, DRM_IOCTL_MODE_GETCONNECTOR, ...);    // K26
ioctl(fd, DRM_IOCTL_MODE_ADDFB2, ...);          // K27
ioctl(fd, DRM_IOCTL_MODE_SETCRTC, ...);         // K27
ioctl(fd, DRM_IOCTL_MODE_CREATE_DUMB, ...);     // K28
ioctl(fd, DRM_IOCTL_MODE_MAP_DUMB, ...);        // K28
void *ptr = mmap(NULL, size, ..., fd, offset);  // K28
for (y = 10; y < 110; y++)                      // ← K29 paints
    for (x = 10; x < 110; x++)
        ((uint32_t*)(ptr + y * pitch))[x] = 0x00FF0000u;
```

Every ioctl returned 0.  The mmap returned a real pointer.
The writes hit physical memory.  And the bytes ended up
visible in the QEMU display window.

## What "K29 means" in one paragraph

Up to K28, Kevlar's DRM stack was a series of ioctl handlers
that recorded state and returned 0.  Userspace got a handle,
mmapped a buffer, wrote pixels — but nothing scanned those
pixels onto a screen.  K29 made the DUMB-buffer pool **be**
Kevlar's existing bochs_fb framebuffer.  Same physical pages.
QEMU's `-device ramfb` was already scanning those pages onto
the host display since boot — for K29 it just so happens that
some of those pages are now also userspace-mappable through
the DRM ioctl path.  Two paths, one buffer, visible pixels.

## The 15 lines of code

The whole K29 patch is 15 lines of Rust + 20 lines of C.
Here's the kernel side:

```rust
pub fn init_dumb_pool() {
    let mut pool = DUMB_POOL.lock();
    if pool.is_some() { return; }

    if bochs_fb::is_initialized() {
        let pa = bochs_fb::phys_addr();
        let size = bochs_fb::size();
        if pa != 0 && size != 0 {
            log::info!(
                "kabi: DUMB pool == bochs_fb: PA={:#x} size={} bytes",
                pa, size,
            );
            *pool = Some(DumbPool {
                base_va: 0,
                base_pa: pa,
                pool_size: size,
                next_offset: 0,
                handles: Vec::new(),
                next_handle: 1,
            });
            return;
        }
    }
    // ... fallback alloc_pages branch unchanged from K28
}
```

That's it.  K28's pool was a fresh `alloc_pages(1024)`; K29
replaces it with bochs_fb's existing 768-page allocation.
Net effect: K29 *saves* 4 MB of kernel memory and *gains*
visible pixels.

## How the layers cooperate

bochs_fb is one of Kevlar's native Rust drivers.  At boot:

1.  `bochs_fb::init()` registers a PCI prober.
2.  On x86_64 the prober finds QEMU's stdvga and configures
    a framebuffer.  On aarch64 (no legacy PCI on QEMU virt),
    main.rs explicitly calls `bochs_fb::init_ram_backed(pa,
    1024, 768, 32)` with a freshly-allocated 3 MB region.
3.  If QEMU has `-device ramfb`, `ramfb::init(fw_cfg, pa,
    1024, 768)` writes the framebuffer's PA to QEMU's
    fw_cfg `etc/ramfb` slot.  QEMU then scans those pages
    onto the host display every refresh tick.

Independently, K29's `init_dumb_pool` reads
`bochs_fb::phys_addr()` and uses that as the pool's base.
Userspace's `mmap(fd, size, ..., offset)` maps the user's VA
to the same phys pages — Kevlar's `sys_mmap` device-memory
path goes through `mmap_phys_base + offset` and lands at
`bochs_fb::phys_addr() + offset`.

When userspace writes to the mmapped region, the bytes go
into the same physical pages QEMU is scanning.  Visible
pixels.

## What's actually mapped where

```
PA  0x7cc00000          ← bochs_fb::phys_addr() == DUMB pool base
                          ramfb scans these pages onto QEMU display.

userspace VA xxxxxxxx   ← mmap'd by drm_ioctl_mode_create_dumb
                          + drm_ioctl_mode_map_dumb + sys_mmap.
                          User writes here go straight to PA.

kernel VA   yyyyyyyy    ← bochs_fb::phys_addr().as_vaddr()
                          The kernel could write here too, but
                          today only userspace touches the
                          framebuffer.
```

Three views of the same 3 MB.  All consistent.  All
addressable from their respective contexts.

## What this is NOT

K29 doesn't make Kevlar's *kABI-loaded* DRM drivers
(cirrus-qemu.ko, bochs.ko) drive a real display.  Those
modules' probe functions ran at K19+ and recorded
internal state, but the visible pixels come from Kevlar's
*native* bochs_fb driver, not from the binary modules.

When the binary cirrus-qemu.ko's `cirrus_pci_probe` was
asked for resource[0] (VRAM BAR), we handed back a 4 KB
zero buffer at PA 0x100000000.  Nothing about that buffer
becomes visible because QEMU's emulated cirrus hardware
isn't actually running anywhere — we have no real PCI bus,
no actual emulated cirrus device.

The kABI path provides the *interface* userspace expects
(opening /dev/dri/card0, walking DRM resources, allocating
DUMB buffers).  bochs_fb provides the *backing* (the
physical pages QEMU actually scans).  K29 just connected
them at the dumb-pool layer.

## What "graphical ASAP" looked like

29 milestones:

- K1-K9: load `.ko` files, satisfy the linker.
- K10-K12: arm64 ABI quirks (SCS, x18-fixed-Rust).
- K13-K18: DRM helper modules + kABI surface explosion.
- K19-K23: probe firing — first running driver code.
- K20-K22: /dev/dri/card0 visible to userspace.
- K23-K24: /dev/input/event2 from the input side.
- K25-K27: modesetting handshake (GETRESOURCES → SETCRTC).
- K28: real DUMB buffer + mmap.
- **K29: visible pixels.**

Six DRM ioctls in K1-K9's stub bucket are still ENOTTY:
PAGE_FLIP, ATOMIC, GEM-anything-else, etc.  But the core
allocate-map-draw-display sequence works.  Every byte
userspace writes ends up at QEMU's display.

## Cumulative kABI surface (K1-K29)

~331 exports.  ~50 shim modules.  Six DRM modules loadable.
**Eight DRM ioctls succeed** end-to-end with real bytes
flowing.  Two userspace-visible char devices
(`/dev/dri/card0`, `/dev/input/event2`) created by kABI
registration.  One end-to-end pixel pipeline from a Linux
binary's userspace ioctl to a host display window.

## Status

| Surface | Status |
|---|---|
| K1-K28 | ✅ |
| K29 — visible pixels | ✅ |
| K30+ — Linux desktop apps on the kABI surface | ⏳ |

## What's next

This is where the kABI arc inflects again.  The original goal
was "load Linux binaries; run them; eventually run a
graphical desktop."  K29 demonstrates the bottom of the stack
works: a Linux userspace binary went through a Linux-shaped
DRM ioctl interface and got a Linux-shaped framebuffer that's
scanned to a host display.

The remaining gap to "Linux desktop running" is on the
**userspace** side — Xorg, libdrm, Mesa, fbdev tools — not
the kernel side.  Kevlar already runs Alpine LXDE through
its native Rust drivers (`make run-alpine-lxde`).  The
question for K30+ is whether to (a) keep using the native
path and treat the kABI arc as parallel, (b) migrate the
LXDE userspace to the kABI path, or (c) something else.

We'll figure that out next post.
