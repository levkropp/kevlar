# M11: Alpine Linux (Graphical)

**Goal:** Boot Alpine Linux with a graphical desktop environment,
keyboard + mouse input, and at least one GUI application (terminal
emulator + web browser).

**Prerequisite:** M10 (Alpine text-mode with networking and apk).

## Why Graphical Matters

A graphical desktop is the proof that Kevlar can replace Linux for
end-user workloads. GPU drivers (virtio-gpu), input devices
(virtio-input), display protocols (Wayland/X11), and compositors
(Weston/XFCE) exercise every kernel subsystem simultaneously:
memory management, process scheduling, I/O multiplexing, IPC,
and hardware abstraction.

## Current State (Post-M10)

Expected working:
- Alpine boots with OpenRC, networking, apk
- virtio-net, virtio-blk drivers
- TCP/UDP sockets, SSH

Missing for graphical:
- No framebuffer / GPU driver
- No DRM/KMS subsystem
- No input device drivers (keyboard/mouse as input events)
- No /dev/fb0, /dev/dri/card0, /dev/input/event*
- No shared memory (SHM) for Wayland/X11 buffer passing

## Phases

| Phase | Deliverable | Est. |
|-------|-------------|------|
| 1: Framebuffer | virtio-gpu or bochs-vga framebuffer, /dev/fb0, fbdev mmap | 2-3 weeks |
| 2: Input | virtio-input keyboard + mouse, /dev/input/event*, evdev | 1-2 weeks |
| 3: Display server | X11 (Xorg + fbdev driver) or Wayland (Weston + DRM) | 2-3 weeks |
| 4: Desktop | XFCE4 or Sway, terminal emulator, basic apps | 2-3 weeks |
| **Total** | | **~2-3 months** |

## Risk Assessment

- **Phase 1 (framebuffer):** Medium risk. virtio-gpu has a well-documented
  spec. The bochs-vga fallback is simpler (just PCI BAR memory-mapped
  framebuffer). Either gives us /dev/fb0 for fbdev-based X11.

- **Phase 2 (input):** Low risk. virtio-input exposes keyboard/mouse as
  simple event streams. The evdev interface is straightforward.

- **Phase 3 (display server):** High risk. Xorg expects DRM/KMS or at
  minimum fbdev. Running Xorg with fbdev is the simplest path. Wayland
  with Weston is cleaner but needs more DRM infrastructure.

- **Phase 4 (desktop):** Medium risk. Once X11/Wayland works, XFCE4
  is a standard `apk add xfce4` install. The risk is in missing syscalls
  or kernel interfaces that surface only under complex GUI workloads.
