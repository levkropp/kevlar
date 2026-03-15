# M11 Phase 5: Desktop Environment

**Goal:** Full desktop with window management, taskbar, settings panel.

## Desktop Options

**XFCE4** (~50MB) — lightest full desktop. Uses X11 (works with Phase 3)
or can use XWayland on Wayland.

**LXQt** (~40MB) — Qt-based lightweight desktop. Needs X11 or Wayland.

**Sway** (~20MB) — i3-compatible Wayland tiling WM. Needs Phase 4 (DRM).

For widest compatibility, target XFCE4 first (X11), then Sway (Wayland).

## Requirements

### D-Bus

XFCE4 and most desktop environments need D-Bus for IPC:
- `dbus-daemon` runs as a system service
- AF_UNIX socket at `/var/run/dbus/system_bus_socket`
- SCM_RIGHTS for fd passing (Phase 4)
- Applications connect and exchange messages

Minimal D-Bus needs:
- Unix sockets (done)
- `sendmsg`/`recvmsg` with ancillary data (SCM_RIGHTS from Phase 4)
- `poll`/`epoll` on Unix sockets (done)

### Shared memory

X11 SHM extension and Wayland SHM protocol need:
- `shm_open()` → creates tmpfs file at `/dev/shm/`
- `mmap(MAP_SHARED)` → shared memory between processes
- `ftruncate()` → set size

MAP_SHARED requires shared page table entries — the same physical page
mapped in multiple processes' address spaces. This is new for Kevlar
(currently all mmap is MAP_PRIVATE).

### Fonts

Desktop needs font rendering:
- `/usr/share/fonts/` with TTF/OTF files
- fontconfig reads font directories
- freetype2 renders glyphs
- All in userspace — no kernel support needed

## Verification

```
apk add xfce4 xfce4-terminal dbus
dbus-daemon --system
startxfce4
# Desktop with taskbar, right-click menu, terminal
```
