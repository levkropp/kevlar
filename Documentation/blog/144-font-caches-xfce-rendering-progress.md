# Blog 144: Fonts, caches, and the invisible desktop

**Date:** 2026-04-05
**Milestone:** M11 Alpine Graphical — Phase 7 (Desktop Rendering)

## Summary

The XFCE desktop components (xfce4-session, xfce4-panel, xfwm4) all
start as processes, but the screen remains black. Investigation revealed
three layers of issues: missing bitmap font XLFD names in `fonts.dir`,
missing GTK3 cache files (gdk-pixbuf loaders.cache), and an ip=0 crash
that kills X11 clients. The first two are fixed. The third is a deeper
kernel bug under active investigation.

## The invisible desktop

With 9/10 XFCE tests passing (session, panel, D-Bus, Xorg, xdpyinfo
all running), the QEMU window showed only a black screen with a mouse
cursor. QEMU screendumps confirmed: Xorg renders its cursor (the
framebuffer mmap works), but no X11 client content is visible.

## Issue 1: Wrong font names in fonts.dir

xterm crashed with:
```
xterm: cannot load font "-misc-fixed-medium-r-semicondensed--13-120-75-75-c-60-iso10646-1"
```

Then SIGSEGV at ip=0 — Xlib's error handler was a NULL function
pointer.

**Root cause:** The image builder generated `fonts.dir` with generic
XLFD names for ALL fonts:
```
6x13.pcf.gz -misc-fixed-medium-r-normal--0-0-0-0-c-0-iso8859-1
```

Instead of the correct name:
```
6x13.pcf.gz -misc-fixed-medium-r-semicondensed--13-120-75-75-c-60-iso10646-1
```

The X server loaded `fonts.dir` but couldn't match xterm's font
request because the XLFD names were wrong. `mkfontdir` inside QEMU
also produced generic names — the tool couldn't parse PCF font
properties (likely an ext2 file read issue).

**Fix:** Pre-generate `fonts.dir` in the image builder with known
XLFD mappings for the 21 standard misc-fixed fonts. Also generate
ISO-8859-1 aliases for each ISO-10646-1 font (356 total entries).

## Issue 2: Missing GTK caches

Even with fonts working, XFCE rendered as transparent/black. GTK3
needs several cache files that Alpine's `apk` triggers create during
package installation. Since our image builder runs `apk` with `--root`
(not in a chroot), the post-install triggers fail and no caches are
generated.

**Required caches:**
- `gdk-pixbuf loaders.cache` — tells GTK which image format loaders
  are available (PNG, JPEG, SVG, etc.). Without it, GTK can't load
  ANY images, including icons and theme assets.
- `fontconfig cache` (`fc-cache`) — indexes available fonts for
  applications using FreeType/fontconfig.
- `mime.cache` — MIME type database for content type detection.

**Fix:** Pre-generate all three in the image builder:
- `loaders.cache`: enumerate loader .so modules and create entries
  mapping each to its MIME type (12 modules).
- `fc-cache`: run host's `fc-cache --sysroot` to index fonts.
- `mime.cache`: run host's `update-mime-database`.

**Note:** Running `fc-cache` inside QEMU triggers a separate ip=0
crash (see Issue 3), so it must be pre-generated on the host.

## Issue 3: ip=0 SIGSEGV in X11 clients

Multiple processes crash with null function pointer calls:

```
SIGSEGV: null pointer access (pid=37, ip=0x0, fsbase=0xa0014bb28)
  RAX=0x0 RBX=0x8001 RCX=0xa1090b010 RDX=0x8001
  [rsp+0x0] = 0x0000000a00107ccb  ← valid return addr in xterm
  [rsp+0x10] = 0x0000000a1048cc1e ← valid return addr in library
```

The crash is `call *%rax` with `RAX=0`. The return address on the
stack IS valid (inside xterm and a library), meaning the call
instruction itself executed correctly but called a NULL target.

**Affected processes:** xterm, dbus-daemon (session), xfce4-panel,
at-spi-bus-launcher. All use Unix sockets for X11 or D-Bus IPC.

**Not the cause:**
- Sockaddr buffer overflow (fixed in Blog 143)
- Font loading failure (fixed above)
- sched_affinity compat (122/123 added)
- Missing syscalls (none reported)

**Under investigation.** The crash pattern (NULL GOT/PLT entry or
corrupted callback pointer) suggests either:
1. A kernel bug in socket communication that corrupts client memory
2. A shared library mapping issue under heavy workload
3. A signal delivery bug that corrupts the user stack

## QEMU screenshot integration

Built a Python-based screenshot capture using QEMU's QMP protocol:

```python
s = socket.create_connection(("127.0.0.1", port))
s.send(json.dumps({"execute": "screendump",
       "arguments": {"filename": "/tmp/screen.ppm"}}).encode())
```

Screenshots taken at timed intervals during automated tests confirm:
- Kernel test pattern (dark blue): framebuffer BAR initialized
- Xorg cursor (white arrow): X server rendering works
- xterm window (white rectangle): X client rendering works
  (seen once before the ip=0 crash became consistent)
- XFCE desktop: renders as black (processes running but invisible)

## Syscall additions

| Syscall | Number | Implementation |
|---------|--------|---------------|
| sched_setaffinity | 122 (compat) | No-op stub (accept all CPUs) |
| sched_getaffinity | 123 (compat) | Returns all online CPUs |

musl libc uses the x86_64 "common" syscall numbers (122/123) instead
of the "64-bit" numbers (203/204). Both are valid on Linux but our
dispatch only handled 203/204.

## Test results

```
TEST_PASS mount_rootfs
TEST_PASS dev_fb0_exists
TEST_PASS fb0_ioctl        (1024x768 32bpp)
TEST_PASS dbus_start
TEST_PASS xorg_running
TEST_PASS xdpyinfo
TEST_PASS xterm_running    (starts, then ip=0 crash)
TEST_PASS xfce4_panel_running
TEST_PASS xfce4_session_running
```

## Next steps

The ip=0 crash is the last major blocker. The crash is deterministic
(100% reproduction rate) and affects all X11 clients that use Unix
sockets. Investigation will focus on:

1. **Socket data integrity**: verify that `recvmsg`/`sendmsg` on
   Unix sockets don't corrupt adjacent memory
2. **Library mapping verification**: check that shared library
   .got/.plt sections aren't being overwritten by kernel operations
3. **Signal delivery audit**: verify that signal frame setup doesn't
   corrupt the user stack when multiple signals are pending
