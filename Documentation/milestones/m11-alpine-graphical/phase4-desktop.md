# M11 Phase 4: Desktop Environment

**Goal:** Full desktop with window management, terminal, file manager.

## Desktop Options

**XFCE4** — lightest full desktop (~50MB installed). Needs X11.
```
apk add xfce4 xfce4-terminal
startxfce4
```

**Sway** — i3-compatible Wayland compositor (~20MB). Needs Wayland/DRM.
```
apk add sway foot
sway
```

**Labwc** — lightweight Wayland compositor, even simpler than Sway.

For first graphical boot, XFCE4 on X11/fbdev is the most proven path.

## Applications

Minimum viable desktop:
- Terminal emulator (xfce4-terminal or xterm)
- File manager (thunar or pcmanfm)
- Text editor (mousepad or nano)
- Web browser (netsurf — lightweight, no heavy deps)

```
apk add xfce4-terminal thunar mousepad netsurf
```

## Kernel Changes for Phase 4

- **Audio**: virtio-snd driver for sound output (optional, nice-to-have)
- **Clipboard**: X11 selections work via the X server, no kernel support
- **Drag and drop**: Application-level protocol, no kernel support
- **Resolution changes**: Bochs VGA mode switching via FBIOPUT_VSCREENINFO
- **Hardware cursor**: Optional optimization for mouse pointer rendering

## Verification

Screenshot comparison or automated test:
1. Boot Alpine with XFCE4
2. Open terminal, run `uname -a` → shows Kevlar kernel
3. Open file manager, navigate to `/proc/`
4. Take screenshot (QEMU monitor: `screendump`)

## M11 Complete

With Phase 4 done, Kevlar runs a graphical Alpine Linux desktop with:
- Window management (XFCE4 or Sway)
- Terminal emulator with shell
- File manager
- Network access (web browser)
- Package installation via apk

This proves Kevlar is a viable drop-in Linux kernel replacement for
desktop workloads.
