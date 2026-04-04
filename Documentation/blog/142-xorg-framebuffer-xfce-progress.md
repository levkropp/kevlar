# Blog 142: Xorg runs on a 1024x768 framebuffer — the path to XFCE

**Date:** 2026-04-04
**Milestone:** M11 Alpine Graphical — Phase 5 (XFCE Desktop)

## Summary

Xorg now starts on Kevlar with the Bochs VGA framebuffer driver,
accepts X11 client connections (xdpyinfo succeeds), and renders to a
1024x768x32 display. Getting here required fixing five distinct bugs
across the context switch, page allocator, device filesystem, sysfs,
and syscall layers. The automated test suite scores 6/9, with D-Bus and
Xorg fully operational. XFCE session components are next.

## The bugs

### 1. Context switch tail call (Blog 140)

The Rust compiler tail-call-optimized `do_switch_thread()`, emitting
`jmp` instead of `call`. This destroyed the caller's stack frame before
the context switch could save it, causing 100% crash rate on every
configuration including single-CPU.

**Fix:** `core::arch::asm!("", options(nomem, nostack))` barrier after
the call prevents the tail call optimization.

### 2. XSAVE buffer overflow

The XSAVE instruction writes FPU/SSE/AVX register state to a buffer
during context switches. With 1-page (4096 byte) buffers, XSAVE could
overflow into the adjacent kernel stack page when the CPU had extended
state components enabled. This manifested as RIP=0 crashes specifically
when D-Bus daemon ran (heavy fork/exec creating many processes with
xsave areas adjacent to stacks).

**Diagnosis:** Crash RSP and xsave buffer RDI were on adjacent pages.
Increasing to 2 pages eliminated the crash.

**Fix:** Allocate 2 pages (8192 bytes) for all xsave areas.

### 3. Device file mode bits

`/dev/fb0` and `/dev/input/mice` were created without `S_IFCHR` in
their stat mode — `ls` showed `?---------` (unknown file type) instead
of `crw-rw-rw-`. Xorg's fbdev driver checks `S_ISCHR()` before opening
the device.

**Fix:** Add `FileMode::new(S_IFCHR | 0o666)` to stat implementations.

### 4. Missing iopl syscall

Xorg calls `iopl(3)` at startup to access VGA I/O ports directly via
`in`/`out` instructions. Without it, Xorg hung silently during
initialization — no log file, no error, just a blocked process.

**Fix:** Implement `iopl(2)` by setting the IOPL bits (12-13) in the
saved RFLAGS register on the syscall stack. When the process returns to
usermode, the CPU has IOPL=3 and VGA port access works.

### 5. PCI-to-framebuffer sysfs cross-link

The most subtle bug. Xorg's `libpciaccess` scans
`/sys/bus/pci/devices/` and finds the Bochs VGA at `0000:00:02.0`. It
then looks for the associated framebuffer at:

```
/sys/bus/pci/devices/0000:00:02.0/graphics/fb0
```

Without this cross-link directory, Xorg's fbdev driver couldn't map the
PCI GPU to its framebuffer device. It tried `graphics/fb0` through
`graphics/fb7`, then `graphics:fb0` through `graphics:fb7` — all
returned ENOENT.

**Diagnosis:** Added `open()` syscall logging for PIDs > 10. Traced
every file Xorg tried to open and found the pattern:

```
open: pid=16 path="/sys/bus/pci/devices/0000:00:02.0/graphics/fb0" ENOENT
open: pid=16 path="/sys/bus/pci/devices/0000:00:02.0/graphics:fb0" ENOENT
open: pid=16 path="/sys/bus/pci/devices/0000:00:02.0/graphics/fb1" ENOENT
...
open: pid=16 path="/sys/bus/pci/devices/0000:00:02.0/graphics:fb7" ENOENT
```

**Fix:** Add `graphics/fb0` subdirectory inside the PCI device's sysfs
entry with `dev` and `uevent` files matching `/dev/fb0`.

## What was built

### Sysfs PCI device entry

Complete PCI device representation at
`/sys/bus/pci/devices/0000:00:02.0/`:

| File | Content | Purpose |
|------|---------|---------|
| vendor | 0x1234 | Bochs VGA vendor ID |
| device | 0x1111 | Bochs VGA device ID |
| class | 0x030000 | VGA display controller |
| config | 256-byte binary | Raw PCI config space |
| resource | BAR0 addresses | Memory-mapped regions |
| boot_vga | 1 | Primary display flag |
| graphics/fb0/dev | 29:0 | Framebuffer cross-link |

### Automated test suite

`testing/test_xfce.c` — fully automated end-to-end test:

1. Mount Alpine ext2 disk
2. Verify `/dev/fb0` exists and is a char device
3. Test FBIOGET_VSCREENINFO (resolution, bpp)
4. Test FBIOGET_FSCREENINFO (type, smem_len, stride)
5. Test mmap (direct framebuffer access)
6. Start D-Bus system daemon
7. Start Xorg with fbdev driver
8. Verify xdpyinfo connects to X server
9. Start XFCE session components

### Syscall additions

| Syscall | Number | Implementation |
|---------|--------|---------------|
| iopl | 172 | Set IOPL in RFLAGS for I/O port access |
| ioperm | 173 | Stub (accept silently) |
| getitimer | 36 | Read ITIMER_REAL state |
| membarrier | 324 | Return OK (glib RCU) |
| get_mempolicy | 239 | Return ENOSYS (glib probe) |
| sched_getparam | 142 | Return default priority |
| mlock/munlock | 149-152 | Return OK |
| ioprio_set/get | 251-252 | Return OK |

## Results

```
TEST_PASS mount_rootfs
TEST_PASS dev_fb0_exists
TEST_PASS fb0_ioctl        (1024x768 32bpp)
TEST_PASS dbus_start
TEST_PASS xorg_running     (Xorg alive, ioctls flowing)
TEST_PASS xdpyinfo         (X server accepting connections)
TEST_FAIL xfwm4_running    (XFCE session next)
TEST_FAIL xfce4_panel_running
TEST_FAIL xfce4_session_running
TEST_END 6/9
```

## Investigation tools

This session relied heavily on targeted kernel instrumentation:

- **fb0 open/ioctl logging:** Every `open()` and `ioctl()` on `/dev/fb0`
  logged with PID, showing which process accessed the device and when.
- **Syscall path logging:** `open()` syscall for PIDs > 10 logged the
  exact file path, revealing which sysfs files Xorg tried to access.
- **fb0_probe binary:** Standalone test that mimics Xorg's exact probe
  sequence (stat → open O_RDONLY → open O_RDWR → FSCREENINFO → mmap),
  confirming the device works independently of Xorg.
- **Non-blocking pipe capture:** Fixed `sh_capture()` to use O_NONBLOCK,
  preventing test hangs when child processes close pipes unexpectedly.

## Next steps

The XFCE session components (`xfwm4`, `xfce4-panel`, `xfce4-session`)
need GTK initialization to succeed. Likely blockers:

1. **GTK/GDK initialization:** GTK probes for display capabilities,
   font rendering, and theme loading. Missing syscalls or files will
   cause silent failures.
2. **Session D-Bus:** `startxfce4` launches a session D-Bus instance
   via `dbus-launch`. The session bus needs working abstract Unix
   sockets (already implemented).
3. **Font rendering:** GTK requires fontconfig and FreeType. Font
   caches may need to be pre-generated (`fc-cache -f`).
4. **Icon/theme loading:** XFCE needs icon themes for the panel and
   desktop. Adwaita and hicolor themes are installed.
