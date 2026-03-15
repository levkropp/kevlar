# M11 Phase 7: Audio + Polish

**Goal:** Sound output, clipboard, and daily-driver quality polish.

## Audio

### Driver: virtio-snd or Intel HDA

**virtio-snd** — QEMU's virtio sound device. Cleaner interface than
HDA emulation. Needs virtio transport (we have it for net/blk).

**Intel HDA** — QEMU emulates Intel HDA by default. PCI device with
memory-mapped registers. More complex but doesn't need virtio.

### ALSA kernel interface

Applications use ALSA (`/dev/snd/pcmC0D0p`):
- `open()` the PCM device
- `ioctl(SNDRV_PCM_IOCTL_*)` — configure format, rate, channels
- `mmap()` — map DMA buffer for zero-copy audio
- `write()` or mmap + `ioctl(SNDRV_PCM_IOCTL_SYNC_PTR)` — submit samples

### PulseAudio / PipeWire

Desktop audio goes through a sound server:
- **PulseAudio** — traditional, well-supported
- **PipeWire** — modern replacement, handles audio + video

These are userspace daemons — kernel just needs ALSA interface.

## Clipboard

X11 clipboard uses X selections (inter-client via X server, no kernel
support needed). Wayland clipboard uses `wl_data_device` protocol
(compositor-mediated, needs data pipe between clients).

Both work with existing kernel primitives (pipes, Unix sockets).

## Polish

- **Screen resolution changes** — `DRM_IOCTL_MODE_SETCRTC` with new mode
- **Window resize** — `TIOCSWINSZ` ioctl on PTY (done)
- **Suspend/resume** — ACPI power management (stretch goal)
- **Multi-monitor** — DRM multiple CRTCs (stretch goal)
- **Hardware acceleration** — virtio-gpu 3D (virgl) for smooth desktop

## Verification

```
# Play audio
apk add alsa-utils
speaker-test -c 2 -t sine  # should produce tone in QEMU
# Clipboard
# Copy text in one app, paste in another
# Resolution
xrandr --output Virtual-0 --mode 1920x1080
```

## M11 Complete

With Phase 7, Kevlar runs a graphical Linux desktop suitable for:
- Terminal-based development (vim/nano, git, make)
- Web browsing (Firefox or NetSurf)
- File management (Thunar/PCManFM)
- Audio playback
- Multi-user with login manager

This is daily-driver quality for basic workstation use.
