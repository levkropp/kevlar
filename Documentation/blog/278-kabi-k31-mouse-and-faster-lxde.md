# 278 — kABI K31: working trackpad + faster LXDE on Kevlar

K30 brought up Alpine's LXDE desktop on Kevlar but two
real-world issues showed up the moment you actually used it:

1.  Mouse cursor was rendered, but the host trackpad didn't
    move it.  You could look at the desktop but not click on
    anything.
2.  Boot to usable session was ~30 seconds.  Linux on the same
    QEMU machine does this in ~5–10s.

K31 ships fixes for both, plus a quieter console.

## The actual mouse bug

K30 used QEMU's `virtio-mouse-device`, which sends *relative*
deltas — Linux mouse semantics, where a movement is `+5` on the
X axis and the guest accumulates position.  That works on a
real mouse but the macOS trackpad reports absolute coordinates;
QEMU has to "grab" the host pointer first, and even then the
mapping is awkward.

The straightforward fix is `virtio-tablet-device`, which sends
absolute coordinates over the same virtio-input protocol.  Host
position maps directly to guest cursor — no grab, no first-move
quirk.  One-line change in `tools/run-qemu.py`:

```diff
- "virtio-mouse-device,event_idx=off,indirect_desc=off",
+ "virtio-tablet-device,event_idx=off,indirect_desc=off",
```

Kevlar's `exts/virtio_input` driver doesn't care which one —
the protocol is identical, only the event semantics differ
(`EV_REL`/`REL_X` vs `EV_ABS`/`ABS_X`).  Kevlar's `EVIOCGABS`
ioctl already reports the correct 0..32767 absolute range
that virtio-tablet uses.

Tested.  Cursor still didn't move.

## The actual *actual* mouse bug

The cursor was there because Xorg renders a software cursor
even with no pointer device loaded.  Xorg started.  But
opening the LXDE image's `xorg.conf.d/10-fbdev.conf` showed
the truth:

```
Section "ServerFlags"
    Option "AutoAddDevices" "false"
    Option "AutoAddGPU" "false"
EndSection

Section "InputDevice"
    Identifier "kbd"
    Driver "evdev"
    Option "Device" "/dev/input/event1"
    ...

Section "ServerLayout"
    Identifier "kevlar-lxde"
    Screen "default"
EndSection
```

There was no pointer `InputDevice` section at all.  The mouse
had never had a chance to work — virtio-mouse vs virtio-tablet
was irrelevant; Xorg simply didn't know there was a pointer
device on the system.

Adding a section pointing at `/dev/input/event0` was step one.
Step two was non-obvious: with `AutoAddDevices "false"`, Xorg's
fallback only auto-loads *one* standalone `InputDevice`
section (typically the keyboard).  A second standalone section
without an explicit `ServerLayout` reference never gets
PreInit-ed.  The fix is to list both in `ServerLayout`:

```
Section "ServerLayout"
    Identifier "kevlar-lxde"
    Screen "default"
    InputDevice "kbd" "CoreKeyboard"
    InputDevice "ptr" "CorePointer"
EndSection
```

The previous "duplicate device file" concern that motivated
the bare `ServerLayout` was a different bug — same identifier
listed twice with the same device path.  With unique
identifiers and `AutoAddDevices=false`, Xorg loads each
device exactly once.

Result: trackpad moves the cursor.

(Determining which event node is which: on QEMU virt, MMIO
addresses get assigned low-to-high to `-device` args in
*reverse* order, so the tablet — listed second — gets the
lower address and is walked first by the DTB scanner.  Net:
`/dev/input/event0` is the tablet, `event1` is the keyboard,
`event2` is the kABI fake virtio-input from the K23 walk.)

## Fast-interactive boot path

The 30-second boot to a usable desktop wasn't actually slow
*kernel*.  Most of those seconds were hardcoded test-harness
delays.  `testing/test_lxde.c` runs as PID 1 in
`run-alpine-lxde` and does:

- 1s + 2s + 1s between dbus / openbox / tint2 / pcmanfm
  spawns — fine, they need that
- **15-second straight settle** — for the regression
  suite's component-running checks
- **8-attempt × 1s pixel-visibility retry** — for the
  framebuffer test
- **3s xterm map wait + xterm spawn** — for the
  typed-text-arrived sub-test

Total: ~30s of intentional waiting.  Useful for batch CI
(`make iterate-lxde`); pure noise for an interactive session.

K31 reads `/proc/cmdline` early in `test_lxde` and, when
`kevlar_interactive` is set (which `run-alpine-lxde` adds via
`--append-cmdline`), short-circuits straight from
"pcmanfm started" to a `sleep(60)` keepalive loop after an
8-second settle.  Wall-clock is now ~12s from boot to a
fully-painted desktop.

`make iterate-lxde` and `make test-lxde` skip the
`kevlar_interactive` flag and keep the full sequence — 8/8
TEST_PASS, 96.2% painted framebuffer regression check
unchanged.

(The 8s isn't arbitrary — pcmanfm needs ~6–7s to render the
wallpaper.  At 3s the panel + clock are up but the desktop
background is still unpainted, leaving a black main area.
Asked the user; "blue background returned" was the
confirmation that 8s is enough.)

## Console quieting

`run-alpine-lxde` previously emitted a steady stream of
diagnostic warnings during interactive sessions:

- `TICK_HB: cpu=N tick=… per_cpu=[…]` once per second per
  CPU — the timer-ISR liveness heartbeat from a long-past
  hang investigation
- `PID1_STALL: tick=N gap=Nms cpu=N user_pc=…` twice per
  second — fires whenever PID 1 hasn't been observed running
  for >1s
- `FILL_TRACE`/`FILL_VERIFY: pid=N vaddr=…` on every
  demand-fault in a hot file-offset range — left over from
  K30's xfce4-session corruption hunt

The third was already gated in K30.  K31 gates the other two
behind `TICK_HB_ENABLED` and `PID1_STALL_ENABLED` consts in
`kernel/timer.rs` (default `false`).  PID1_STALL specifically
fires false-positive in the new interactive flow because the
keepalive does `while (1) sleep(60)` — PID 1 is *deliberately*
blocked, not stuck, but the detector can't tell the
difference.

Flip the consts to `true` when investigating a real hang.

## The four (well, six) changes

| File | Change |
|---|---|
| `tools/run-qemu.py` | virtio-mouse → virtio-tablet (absolute pointer) |
| `tools/build-alpine-lxde.py` | add `InputDevice "ptr"` + list both in `ServerLayout` |
| `testing/test_lxde.c` | early `kevlar_interactive` short-circuit; 8s settle; persist Xorg + session log to `/var/log` for post-run diagnosis |
| `kernel/timer.rs` | gate `TICK_HB` + `PID1_STALL` warns behind `*_ENABLED` consts |
| `platform/arm64/mod.rs` | `TICK_HZ` 50 → 100 (matches x64 reference) |

`TICK_HZ 50 → 100` halves the scheduler-quantum latency on
arm64.  Subjectively responsive; objectively the regression
suite is unchanged.

## What's still rough

`run-alpine-lxde` boots, paints the wallpaper, the trackpad
moves the cursor.  Right-click → "Create New Folder" creates
the folder.  But:

- **No icons.**  The new folder appears as a label without
  the standard pcmanfm folder icon.  Icon-theme loading
  probably fails silently — Adwaita is on the disk; the
  question is whether GIO/GdkPixbuf finds it.
- **Double-clicking freezes the desktop.**  pcmanfm forks
  to launch the new folder in its file-manager view, and
  something in that path hangs.  Xorg-side deadlock (likely
  pcmanfm waiting on a D-Bus call that never returns), or
  a Kevlar-side scheduling pathology, or both.
- **Boot to wallpaper is ~12s, vs Linux's ~5–10s.**  Half
  of K30's gap closed; the rest is real kernel-perf work
  (page-fault handler latency, lock contention, virtio-blk
  request batching).
- **Xorg log persistence is best-effort.**  `-logfile
  /var/log/Xorg.0.log` opens the file but Xorg buffers
  internally and the buffer doesn't always flush before
  Kevlar halts.  A graceful shutdown signal to Xorg before
  halt would fix this.

K32 picks up the icon + double-click investigation.  The
freeze in particular is interesting — it's the first sign of
a real userspace workload that doesn't "just work", which
means there's a concrete kernel/userspace interaction bug to
chase rather than just bringing up another component.

## Status

| Milestone | Status |
|---|---|
| K1-K29: kABI arc | ✅ |
| K30: graphical Alpine LXDE | ✅ |
| K31: trackpad + 12s boot + quiet console | ✅ |
| K32+: icons + double-click freeze + sub-Linux boot | ⏳ |
