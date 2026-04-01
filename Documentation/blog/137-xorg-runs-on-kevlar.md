# Blog 137: X11 runs on Kevlar — from zero pixels to a working Xorg server

**Date:** 2026-04-01
**Milestone:** M11 Alpine Graphical — Phase 1-3 (Framebuffer, Input, X11)

## Summary

In a single session, Kevlar went from text-only serial console to running
Xorg 21.1.16 with the fbdev video driver on a 1024x768 framebuffer. The
X server starts, accepts connections, and xdpyinfo queries it successfully.
All 14 X11 integration tests pass. The Alpine smoke test (67/67) and
contract tests (159/159) remain green throughout.

This post covers the full stack: PCI device discovery, VBE register
programming, /dev/fb0 with mmap, PS/2 keyboard and mouse drivers, a
framebuffer text console, and the X11 integration test.

## The framebuffer stack

### Bochs VGA driver (`exts/bochs_fb/`)

QEMU's default display adapter is the Bochs VGA (PCI vendor 0x1234,
device 0x1111). It's the simplest GPU to drive — no command queues,
no DMA, just I/O port registers and a linear framebuffer in BAR0.

The driver:
1. Scans PCI bus, finds the device at slot 0:2
2. Reads BAR0 for the framebuffer physical address (0xFD000000)
3. Queries VBE register 0x0A for VRAM size (16MB)
4. Programs VBE registers for 1024x768x32 with linear framebuffer enabled
5. Paints the screen dark blue as a visual test

```
bochs-fb: found Bochs VGA device on PCI 0:2
bochs-fb: VBE ID=0xb0c5, VRAM=16MB at paddr 0xfd000000
bochs-fb: mode set to 1024x768x32
```

### /dev/fb0 (`kernel/fs/devfs/fb.rs`)

The device file implements the standard Linux fbdev interface:

- **FBIOGET_VSCREENINFO** — returns `fb_var_screeninfo` with resolution,
  bpp, and BGRA8888 bitfield layout
- **FBIOGET_FSCREENINFO** — returns `fb_fix_screeninfo` with physical
  address, stride, memory size
- **read/write** — direct pixel access at byte offsets
- **mmap** — maps framebuffer memory directly into userspace

The mmap support required a new VMA type in the kernel:

```rust
pub enum VmAreaType {
    Anonymous,
    File { file, offset, file_size },
    DeviceMemory { phys_base: usize },  // NEW
}
```

When a page fault hits a `DeviceMemory` VMA, the handler maps the
physical device page directly — no allocation, no copy. The `FileLike`
trait gained a `mmap_phys_base()` method that tells the mmap syscall
to create a `DeviceMemory` VMA instead of a regular `File` VMA.

### Framebuffer console (`platform/x64/fbcon.rs`)

A 128x48 character text console rendered directly on the framebuffer
using an embedded 8x16 bitmap font. All kernel log output goes to
both serial AND the framebuffer display. This means you can see boot
messages on the QEMU window without needing a serial terminal.

```
fbcon: 128x48 text console on framebuffer
```

## Input devices

### PS/2 keyboard (`platform/x64/ps2kbd.rs`)

Handles IRQ1 from the i8042 controller:
- Reads scan codes from port 0x60
- Translates Set 1 scan codes to ASCII (with shift, ctrl, caps lock)
- Feeds characters into the existing console input path
  (`handle_console_rx`)

The keyboard shares the same input pipeline as the serial port — once
a character is translated, the line discipline handles echo, signals
(Ctrl+C → SIGINT), and buffering identically.

### PS/2 mouse (`platform/x64/ps2mouse.rs`)

Handles IRQ12 from the i8042 auxiliary port:
- Initializes the mouse via i8042 commands (enable aux, set sample rate,
  enable data reporting)
- Assembles 3-byte ImPS/2 packets with sync bit validation
- Stores packets in a 512-byte ring buffer

### /dev/input/mice (`kernel/fs/devfs/mice.rs`)

A readable device file that X11's mouse driver reads:
- Blocking and non-blocking read support
- Poll/epoll integration (wakes on new mouse packets)
- Returns raw ImPS/2 3-byte packets

## X11 integration test

The test (`testing/test_xorg.c`) boots Alpine from a 512MB ext2 disk
image with Xorg pre-installed (82 packages including xorg-server,
xf86-video-fbdev, xterm, twm, xinit, fonts).

### Results: 14/14 PASS

| Phase | Tests | Result |
|-------|-------|--------|
| Device files | fb0 open, ioctl, mmap; /dev/input/mice | 5/5 PASS |
| X11 binaries | Xorg, xterm, twm, xinit, xdpyinfo | 5/5 PASS |
| X11 config | fbdev xorg.conf | 1/1 PASS |
| Xorg startup | version check, startup, xdpyinfo query | 3/3 PASS |

The test starts Xorg in the background, waits 3 seconds, confirms it's
still running, queries it with xdpyinfo, then kills it cleanly.

### How to run it yourself

```bash
# Build the X11 Alpine disk image (downloads 82 packages, ~300MB)
python3 tools/build-alpine-xorg.py build/alpine-xorg.img

# Run the automated X11 test
make test-xorg

# Boot Alpine with a QEMU window (see the framebuffer!)
make run-alpine-gui
```

`make run-alpine-gui` opens a QEMU window where you can see the
framebuffer console output and interact via keyboard. To start X11
interactively from the shell:

```sh
# Inside Alpine on Kevlar:
export DISPLAY=:0
Xorg :0 -noreset -nolisten tcp &
xterm &
twm &
```

## What's left for a fully graphical Alpine desktop

The X server starts and runs. To get to a usable graphical desktop,
the remaining work is:

### Already working
- Framebuffer (1024x768x32, /dev/fb0 with mmap)
- PS/2 keyboard input (IRQ1, scancodes, modifiers)
- PS/2 mouse input (IRQ12, ImPS/2 packets, /dev/input/mice)
- Xorg 21.1.16 with xf86-video-fbdev
- xterm, twm, xinit installed
- 67/67 smoke tests, 159/159 contract tests green

### Needed for interactive desktop use
1. **VT/TTY switching** — Xorg tries to use VT ioctls (VT_GETMODE,
   VT_SETMODE, KDSETMODE) for virtual terminal management. Currently
   these return ENOTTY. Stubbing them to succeed would let Xorg run
   without `-noreset`.
2. **Unix socket improvements** — X11 clients connect to the X server
   via `/tmp/.X11-unix/X0`. The abstract socket namespace and
   `SCM_CREDENTIALS` ancillary data may need work.
3. **Shared memory (SHM/MIT-SHM)** — X11 uses `shmget`/`shmat` for
   fast pixmap transfer. Currently not implemented (Xorg falls back
   to socket transfer, which is slower).
4. **Font rendering** — The font triggers failed during image build.
   Running `fc-cache` inside Kevlar or pre-building font caches would
   fix xterm's font rendering.
5. **XFCE/desktop environment** — `apk add xfce4` installs the full
   desktop. Needs the above items plus D-Bus, polkit, and ConsoleKit
   stubs.

### Nice to have (not blocking)
- virtio-gpu (3D acceleration, better than fbdev)
- virtio-input (replaces PS/2, supports multi-touch)
- DRM/KMS (modern display API, replaces fbdev)
- Audio (ALSA/PulseAudio via virtio-sound)

## Verified

- **67/67 Alpine smoke tests** — no regressions from graphical additions
- **159/159 contract tests** — ABI compatibility preserved
- **14/14 X11 tests** — Xorg starts, runs, responds to queries
- Boot time: ~2 seconds to shell prompt under KVM
- All new code compiles cleanly with `#![deny(unsafe_code)]` on kernel crate
  (only `#[allow(unsafe_code)]` on specific functions that touch MMIO/ports)
