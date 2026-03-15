# M11: Graphical Linux Desktop

**Goal:** Boot Alpine or Ubuntu with a full graphical desktop: XFCE4 or
LXQt, terminal emulator, file manager, web browser. End state is a
usable workstation that could replace a daily-driver Linux install for
basic tasks.

**Prerequisite:** M10 (text-mode Linux with ext4, networking, multi-user).

## Why Graphical Matters

A graphical desktop exercises every kernel subsystem simultaneously:
GPU drivers, input handling, memory management (large mmap regions,
shared memory), process scheduling (compositor + apps + services),
IPC (Wayland/X11 sockets, D-Bus), and hardware abstraction. If the
desktop works, the kernel is real.

## Phases

| Phase | Deliverable | Est. |
|-------|-------------|------|
| 1: Framebuffer | Bochs VGA / virtio-gpu framebuffer, /dev/fb0, fbset | 2-3 weeks |
| 2: Input | PS/2 or virtio-input, evdev /dev/input/event*, libinput | 1-2 weeks |
| 3: X11 minimal | Xorg + fbdev driver, xterm, basic window management | 2-3 weeks |
| 4: Wayland + DRM | DRM/KMS subsystem, Weston or Sway compositor | 3-4 weeks |
| 5: Desktop environment | XFCE4 or LXQt, file manager, settings | 2-3 weeks |
| 6: Applications | Terminal, web browser (Firefox/NetSurf), text editor | 2-3 weeks |
| 7: Audio + polish | virtio-snd / Intel HDA, PulseAudio/PipeWire, clipboard | 2-3 weeks |
| **Total** | | **~4-5 months** |

## Phase Progression

```
Phase 1-2: Pixels on screen + keyboard/mouse input
Phase 3:   First GUI window (X11, proven path, works with fbdev)
Phase 4:   Modern display stack (Wayland, needed for Sway/GNOME/KDE)
Phase 5-6: Usable desktop with real applications
Phase 7:   Audio, polish, daily-driver quality
```

## Key Kernel Subsystems Needed

| Subsystem | Phase | Complexity |
|-----------|-------|------------|
| PCI BAR mapping (framebuffer) | 1 | Medium |
| fbdev (/dev/fb0) | 1 | Low |
| evdev (/dev/input/event*) | 2 | Medium |
| DRM/KMS (/dev/dri/card0) | 4 | High |
| MAP_SHARED mmap | 3 | Medium |
| SCM_RIGHTS (fd passing) | 4 | Medium |
| POSIX shared memory (shm_open) | 3 | Low |
| eventfd / signalfd (compositor) | done | — |
| VT switching ioctls | 3 | Low (stub) |
| ALSA or virtio-snd | 7 | High |
| D-Bus (AF_UNIX + SCM_RIGHTS) | 5 | Medium |
