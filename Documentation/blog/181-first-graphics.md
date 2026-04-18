## Blog 181: first pixels — Xorg and kxserver rendering on Kevlar

**Date:** 2026-04-19

The CLD fix from [blog 180](180-the-cld-bug.md) took the XFCE
kernel-crash rate to 0. With the kernel stable, we can finally see
what Kevlar looks like with a framebuffer driver and an X server on
top of it.

## Xorg fbdev + xsetroot

The simplest possible graphical test:

1. Boot Kevlar
2. Mount an Alpine rootfs
3. Start Xorg with the `fbdev` driver pointing at `/dev/fb0`
4. `xsetroot -solid '#336699'` to paint the root window

`testing/test_x11_visible.c` does exactly this, then `sleep(60)`s
forever so `tools/xfce-screenshot.py` can capture the framebuffer via
QEMU's QMP `screendump` command.

![Xorg fbdev rendering a #336699 root window](images/181-xorg-blue.png)

Every pixel is `srgb(51, 102, 153)` — exactly the color `xsetroot`
asked for. End-to-end path:

```
Kevlar kernel
  → Xorg (fbdev driver)
  → /dev/fb0 (framebuffer device)
  → Bochs VGA emulation
  → QEMU VGA surface
  → QMP screendump → PPM → PNG
```

Four weeks ago this path would crash before xsetroot could even
connect to Xorg. Now it's deterministic.

## kxserver + xterm window

The more interesting path: [kxserver](174-kxserver-phase-12-real-clients-gtk3.md),
our own Rust X11 server, running directly on Kevlar with no Xorg
at all. kxserver talks directly to `/dev/fb0`, implementing the
X11 wire protocol in ~10 kloc of Rust.

`testing/test_kxserver_visible.c` starts `kxserver :1`, runs
`xsetroot -solid '#2266aa'` against it, then launches xterm:

![kxserver rendering an xterm window on Kevlar](images/181-kxserver-xterm.png)

The 80×24 xterm window is painted at `+50+50` with its configured
navy background `#001122`. The surrounding light blue `#2266aa` is
the root window that xsetroot painted. The text content inside isn't
visible in this capture because xterm SIGSEGVs right after `MapWindow`
(probably a font or pty issue on this minimal Alpine image) — but
the window frame is real, kxserver handled the `CreateWindow`,
`MapWindow`, and `FillRectangle` requests correctly, and the pixels
landed in the framebuffer.

This is end-to-end:

```
Kevlar kernel
  → xterm (glibc X11 client)
  → Unix domain socket to kxserver
  → kxserver (Rust, direct framebuffer)
  → /dev/fb0
  → Bochs VGA → QEMU → PNG
```

No Xorg, no fbdev driver stack, no mesa, no DDX. Just a Rust server
and the kernel.

## What it took

- **Boot stability**: CLD fix (blog 180). Without it, 37% of runs
  panic before XFCE components have finished initializing.
- **Fault-safe fault handler**: when a user process faulted with an
  unmapped RSP, the kernel's own fault-dump code would crash trying
  to read `*(rsp as *const u64)`. Fixed by routing the dump through
  `UserVAddr::read_bytes`, which uses the fault-safe `copy_from_user`
  asm path.
- **Screenshot harness**: `tools/xfce-screenshot.py` boots with QMP
  exposed and captures the framebuffer at intervals. Same e_machine
  patch (`EM_X86_64` → `EM_386`) as `run-qemu.py` to bypass SeaBIOS's
  unreliable linuxboot.rom path.

## What's next

Two immediate paths to real XFCE:

1. **Fix the XFCE userspace crashes.** `xfce4-session` SIGSEGVs in
   ~20% of runs; when launched directly (bypassing session), `xfwm4`,
   `xfdesktop`, and `xfsettingsd` all SIGSEGV at startup. These are
   probably missing syscall semantics we haven't implemented yet —
   each one needs a look at the strace.
2. **Grow kxserver to handle xterm's full protocol set.** The window
   appears; the text doesn't. That's probably the font-loading path
   in xterm tripping on something kxserver doesn't handle yet. But
   we already have GTK3 working against kxserver on the host side
   (blog 174), so this is incremental.

The kernel is no longer the bottleneck.
