# M11 Phase 3: Display Server

**Goal:** X11 or Wayland display server running with window management.

## Approach: X11 with fbdev (simplest)

Xorg can use the generic `fbdev` driver which just mmaps `/dev/fb0`.
No DRM/KMS required. This is the quickest path to a working GUI.

```
apk add xorg-server xf86-video-fbdev xterm xinit
startx
```

### Xorg requirements

- `/dev/fb0` (from Phase 1) — framebuffer mmap
- `/dev/input/event*` (from Phase 2) — keyboard + mouse via evdev
- Unix sockets — X11 client-server communication via `/tmp/.X11-unix/X0`
- `shm_open` / `mmap(MAP_SHARED)` — shared memory for SHM extension
- `select`/`poll`/`epoll` — event multiplexing
- `/dev/tty*` — VT switching (can be stubbed)

### Missing kernel features

- **MAP_SHARED mmap**: Our mmap is MAP_PRIVATE only. X11's SHM extension
  and Wayland buffer sharing need MAP_SHARED for zero-copy rendering.
  This requires shared page table entries between processes.

- **Unix socket SCM_RIGHTS**: Passing file descriptors between processes
  via `sendmsg()/recvmsg()` with `SCM_RIGHTS` ancillary data. Wayland
  uses this for buffer passing. X11 can work without it (uses SHM).

- **VT switching ioctls**: `VT_SETMODE`, `VT_ACTIVATE`, `VT_WAITACTIVE`.
  Xorg expects these. Can be stubbed for single-seat systems.

## Alternative: Wayland with Weston

Weston is Wayland's reference compositor. It needs:
- DRM/KMS for display management (more complex than fbdev)
- GBM for buffer allocation
- EGL for rendering
- libinput for input

This is a longer path but produces a cleaner architecture. Phase 3 can
start with X11/fbdev and migrate to Wayland in Phase 4.

## Verification

```
# In Alpine with X11 installed:
startx -- -retro  # starts X11 with a root window pattern
# xterm should appear with a shell prompt
```

Success: graphical window with xterm visible in QEMU display.
