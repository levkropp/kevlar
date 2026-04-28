# 277 — kABI K30: graphical Alpine LXDE on Kevlar

`make ARCH=arm64 run-alpine-lxde` now opens a QEMU window with
a working LXDE desktop on a Rust microkernel.

![K30 LXDE](k30-lxde.png)

That's a real frame from a Kevlar boot.  Xorg + openbox + tint2
+ pcmanfm — full Alpine Linux userspace — running on top of
Kevlar's native bochs_fb framebuffer driver, scanned out by
QEMU's `-device ramfb`.  Blue wallpaper, mouse cursor, "desktop
1" workspace label, clock showing **11:30 / Tuesday 28 April**.
96.2% of the 1024×768 framebuffer painted (756,733 / 786,432
pixels).

## What this demonstrates

Kevlar runs Alpine Linux's complete userspace graphical stack:

- Init: `/bin/test-lxde` (PID 1)
- Display server: Xorg with the `xf86-video-fbdev` driver
  writing to `/dev/fb0`
- Input: `xf86-input-evdev` reading from `/dev/input/event0`
  (virtio-keyboard) and `event1` (virtio-mouse)
- Window manager: openbox 3.6
- Panel: tint2
- File manager / desktop: pcmanfm
- D-Bus: dbus-daemon (system + session)
- Linux userspace API: Alpine 3.21 musl libc, fully unmodified
  Linux ELF binaries

All of this runs on Kevlar — a Rust microkernel that started as
a hobby project and now hosts an actual graphical Linux desktop
session.

## How the pieces line up

K1-K29 was the **kABI arc**: load Linux binary `.ko` modules,
satisfy their linker symbols, fire their probe functions,
dispatch their ioctls.  Five DRM helper modules + two real DRM
drivers + virtio_input.ko all load and run inside Kevlar.  K29
demonstrated visible pixels through that path.

K30 is the **other path**: Kevlar's native Rust drivers
(`exts/bochs_fb`, `exts/ramfb`, `exts/virtio_input`) provide
exactly what userspace needs through the standard
`/dev/fb0` + `/dev/input/event*` interfaces.  Xorg's fbdev +
evdev drivers don't care that the kernel under them is Rust;
they read and write the same character devices Linux would.

Both paths converge at the same physical pages — K29's
"DUMB pool == bochs_fb" wiring proves the kABI side and the
native side share buffers.  But for **graphical Alpine ASAP**,
the native path is enough.  The kABI arc is parallel
infrastructure, not a prerequisite.

## What `make run-alpine-lxde` actually does

```
make build INIT_SCRIPT=/bin/test-lxde
qemu-system-aarch64 -kernel kevlar.arm64.img \
    -append "kevlar_interactive" \
    -drive file=alpine-lxde.arm64.img,if=none \
    -device virtio-blk-device,drive=hd0 \
    -device ramfb \
    -device virtio-keyboard-device \
    -device virtio-mouse-device \
    -display [cocoa/sdl/gtk]
```

Boot:

1.  Kevlar comes up, mounts the initramfs.
2.  PID 1 = `/bin/test-lxde` from the initramfs.
3.  test-lxde mounts the Alpine ext2 disk at `/mnt`.
4.  chroots into `/mnt` and starts Xorg + openbox + tint2 +
    pcmanfm + dbus.
5.  Runs 8 sub-tests verifying the desktop is up.
6.  Reads `/proc/cmdline` — sees `kevlar_interactive` —
    sleeps forever.

Userspace stays alive in the QEMU window.

## Two small fixes that were on the path

K30 wasn't free — there were three small fixes between "the
infrastructure exists" and "you can actually run it":

**FILL_TRACE warn! spam.**  `kernel/mm/page_fault.rs` had a
diagnostic from a long-fixed xfce4-session corruption.  Every
demand-fault for `offset_in_file ∈ [0x1d000, 0x1f000]` (a hot
range) emitted `FILL_TRACE: pid=N vaddr=…` and `FILL_VERIFY: …`
at `warn!` level.  Gated behind a `FILL_TRACE_ENABLED` const
(default false).  Re-enable if the corruption pattern ever
recurs.

**boot-alpine path panics.**  The standard busybox-init flow
(`make build INIT_SCRIPT=/bin/boot-alpine`) hits an
`Option::unwrap()` on `current.vm()` at `page_fault.rs:674`
about 6 seconds into boot.  Real bug, separate from K30.
Workaround: `run-alpine-lxde` now uses `/bin/test-lxde` (which
brings up the same desktop reliably) with the new
`kevlar_interactive` cmdline flag so the test process sleeps
instead of exiting.

**iterate-lxde verdict was always wrong.**  `tools/iterate-lxde.py`
imported PIL inside a try/except that silently fell back to
"0 non-black pixels" when Pillow wasn't installed.  Pillow
*was* in `/opt/homebrew`, but the Makefile's `PYTHON3 = uv run
python` ran inside an isolated uv env without it.  Add Pillow
to `pyproject.toml`'s deps; `uv sync` pulls it.  Also replaced
the per-pixel Python loop with PIL's split/merge + histogram
counting (10 seconds → 36 ms).

After those fixes, `make iterate-lxde` reports the truth:

```
TEST_PASS mount_rootfs / xorg_running / openbox_running /
          tint2_running / pcmanfm_running / lxde_pixels_visible /
          evdev_event0_present / evdev_event0_readable
TEST_END 8/8

framebuffer: 756733/786432 non-black pixels (96.2%) →
             build/lxde-iteration.png
VERDICT: framebuffer is being drawn (>50% non-black).
```

## What's still rough

The desktop comes up but it's slow — about 30 seconds from
boot to a usable terminal window.  Linux on the same hardware
would do this in 5-10 seconds.  Likely culprits: scheduler
quanta, missed interrupt deliveries, page-fault handler busy-
waits, virtio-blk request batching.  None of these are
graphical-stack issues; they're general kernel-perf items
that show up most clearly under userspace load.

Mouse input from the macOS trackpad doesn't reach Xorg yet
(the cursor doesn't track host pointer movement).  This may
be related to the same scheduling issue, or to virtio-mouse
event polling that's missing during userspace activity.  Easy
to investigate from the existing virtio_input plumbing.

## Status

| Milestone | Status |
|---|---|
| K1-K29: kABI arc | ✅ |
| K30: graphical Alpine LXDE running | ✅ |
| K31+: perf + input + boot-alpine fix | ⏳ |

The "graphical ASAP" goal that started 30 milestones ago is
done.  Linux's desktop userspace renders on a Rust kernel —
not via emulation, not via translation, but as native
processes opening native character devices.  The kernel
underneath is unfamiliar; the userspace doesn't notice.
