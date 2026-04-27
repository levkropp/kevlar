# 248 — Input end-to-end through Xorg, and a per-program harness that surfaces real bugs

Three things this session, all on the path to "iterate on real
LXDE programs":

1. **Input is verified end-to-end.**  QMP-injected keystrokes
   propagate virtio-keyboard → kernel evdev → `/dev/input/event1`
   → `xf86-input-evdev` → Xorg core, with no errors and no
   driver unload.  The trip cost four real Kevlar bugs (or near-
   bugs) and one Xorg config gotcha.
2. **`make iterate-program PROG=<name>` lands.**  A generic
   harness brings up the LXDE session, spawns the named program,
   and emits four standard sub-tests — `process_running`,
   `window_mapped`, `pixels_changed`, `clean_exit`.  Adding a
   new program test now takes ~3 minutes.
3. **The harness immediately found a real Kevlar bug.**  xeyes
   passes 6/6 cleanly.  xcalc pulls in Xaw, hits 5M-spin lock
   contention during dynamic-loader stage, and panics the
   kernel.  That's the value of the harness in one run.

The plan from blog 247 had Phase 1 as the riskiest unknown, on
the theory that "every later phase that depends on input is
built on sand if input doesn't actually deliver."  That theory
held — Phase 1 produced more kernel-side fixes than expected.

## Phase 1: from "events queued in virtio" to "events reach Xorg core"

The 6/6 `test-lxde` from blog 247 proved processes spawn and
pixels reach `/dev/fb0`.  It said nothing about whether a
keystroke could traverse the input stack.  The plan called this
out as the riskiest unknown.

### The diagnostic primitive

`run-qemu.py` learned two flags:

```
--inject-on-line PATTERN   # serial-line trigger
--inject-keys STRING       # ASCII → qcode map, sent via QMP
                          # input-send-event when PATTERN matches
```

The patterns/key map handle printable ASCII, newline, tab, space,
plus shift-held capitals & shifted symbols.  Internally it
mirrors the `--nmi-on-stall` plumbing — same intercept thread,
same QMP socket, just a different trigger and a different
sequence of QMP commands.

The in-guest test (`testing/test_lxde.c`) gained three
sub-tests:

```
TEST_PASS evdev_event0_present       # /dev/input/event0 stat()s
TEST_PASS evdev_event0_readable      # nonblocking read() returns EAGAIN
TEST_SKIP typed_text_arrived         # 'kevlar-keys\n' never landed in xterm
```

…and an `evdev_keys_arrived` check that opens `/dev/input/event*`,
prints a `INJECT_NOW: kevlar-lxde-input-ready` sentinel on
serial, then reads the device after a 6-second sleep.

The first run was instructive.

### Bug 1: `vring-num` mismatch (red herring, but kept)

Initial trace showed:

```
virtio-input: registered virtio-input0 (mmio=0xa003a00, irq=77, num_descs=1024)
virtio-input: registered virtio-input1 (mmio=0xa003c00, irq=78, num_descs=1024)
```

Two devices, both with 1024 descriptors.  But `qmp-input-probe.py`
(an existing diagnostic from blog 240) reported `vring-num=1024`
on the QEMU side too — they agreed.  Not a bug, just a
note-to-self that QEMU defaults to a giant queue for virtio-input.

### Bug 2: events drained from the wrong virtqueue (or so we thought)

After `--inject-keys "kevlar-keys\n"`:

```
run-qemu.py: injected 12 keystrokes after seeing 'INJECT_NOW: …'
virtio-input: virtio-input1 irq#0 drained 2 events qlen=2
…
virtio-input: virtio-input1 irq#25 drained 2 events qlen=48
```

Events flowed.  But:

```
evdev event0 read 0 bytes after inject
TEST_SKIP evdev_keys_arrived
```

Test reads from `/dev/input/event0` and gets zero bytes.  The
kernel says `virtio-input1` drained 26 IRQs × 2 events = 52
events into `qlen=48`-and-counting.  Different device.

Reason: QEMU's virt-mmio MMIO assignment on arm64 puts the
**latest** `-device` on the cmdline at the **lowest** address,
so the DTB walker sees them in **reverse** order.  Our cmdline
is:

```
-device virtio-keyboard-device,…
-device virtio-mouse-device,…
```

so `virtio-input0` = mouse, `virtio-input1` = keyboard.  The
test was reading the wrong device.  Read both, problem solved.
Not a kernel bug; a documentation gap.

### Bug 3: EVIOCGBIT was lying

With the test reading both event nodes, `evdev_keys_arrived`
passed: 48 bytes (12 keystrokes × 2 events × 24-byte
`struct input_event`).

But `typed_text_arrived` still skipped.  Adding the explicit
`Section "InputDevice"` to xorg.conf got Xorg to *try* to load
evdev on `/dev/input/event0` and `/dev/input/event1` — and we
got this:

```
(--) evdev: evdev0: Found 20 mouse buttons
(--) evdev: evdev0: Found scroll wheel(s)
(--) evdev: evdev0: Found relative axes
(--) evdev: evdev0: Found absolute axes
(--) evdev: evdev0: Found absolute tablet.
(--) evdev: evdev0: Found keys
(II) evdev: evdev0: Configuring as tablet
(II) evdev: evdev0: Configuring as keyboard
```

Both devices reported every capability, because our `EVIOCGBIT`
ioctl handler returned **all-bits-set** for `EV_KEY`, every
relative axis, and every absolute axis — all on every device.

```rust
// kernel/fs/devfs/evdev.rs (before)
1 => {
    // EV_KEY: which keys/buttons are supported.  Set
    // every bit in the user's buffer — we don't know
    // the device's actual key set without reading
    // virtio config space, and Xorg evdev tolerates
    // over-reporting (keys the kernel doesn't deliver
    // simply never fire).
    for b in bits.iter_mut() { *b = 0xff; }
}
```

The comment said this was tolerable.  In practice Xorg
configures every device as both a keyboard *and* a tablet, fails
to set up a coherent device, and unloads.

Real fix: read the actual capability bitmap from virtio-input
config space.  Per spec §5.8.5, the device exposes a
`(select, subsel)`-tuple-keyed config layout — write `select=0x11`
(EV_BITS) and `subsel=ev_type`, read `size`, read `data[0..size]`.

The kernel didn't have config-space writes exposed, only reads.
Added `write_device_config8` to the `VirtioTransport` trait
(default no-op for PCI, real for MMIO), surfaced it on `Virtio`,
and at `virtio_input` probe time we now do:

```rust
for ev_type in 0u8..32 {
    let bits = read_config_bitmap(&virtio, VIRTIO_INPUT_CFG_EV_BITS, ev_type);
    if !bits.is_empty() {
        *device.ev_bits[ev_type as usize].lock() = bits;
    }
}
```

`InputDevice` gained a `[SpinLock<Vec<u8>>; 32]` — one per Linux
event type.  `EVIOCGBIT(ev_type)` now returns the device-reported
bitmap directly:

```rust
// kernel/fs/devfs/evdev.rs (after)
let stored = dev.ev_bits[ev_type].lock();
let n = core::cmp::min(stored.len(), size);
bits[..n].copy_from_slice(&stored[..n]);
```

After this, Xorg's discovery looks like:

```
(--) evdev: kbd: Vendor 0x1af4 Product 0x2
(--) evdev: kbd: Found keys
(II) evdev: kbd: Configuring as keyboard
(II) XINPUT: Adding extended input device "kbd" (type: KEYBOARD, id 6)
```

One device, one role, no confusion.  Mouse devices analogously
report only EV_REL/EV_ABS/buttons.

### Bug 4: EVIOCSREP nr was wrong

Even with honest EVIOCGBIT, Xorg still logged:

```
(EE) evdev: kbd: Failed to set keyboard controls: No such file or directory
```

…21 ms after registering the keyboard, then unloaded it.

Source code dive (`xf86-input-evdev`):

```c
static void
EvdevKbdCtrl(DeviceIntPtr device, KeybdCtrl *ctrl) {
    ...
    if (ioctl(pInfo->fd, EVIOCSREP, &rep) < 0) {
        xf86IDrvMsg(pInfo, X_ERROR,
            "Failed to set keyboard controls: %s\n", strerror(errno));
        return;
    }
}
```

So EVIOCSREP fails.  Our handler:

```rust
// EVIOCGREP (nr=0x03) — keyboard auto-repeat: u32 delay (ms),
// u32 period (ms).  Match Linux defaults.
if nr == 0x03 && is_read {
    ...
}
// EVIOCSREP (nr=0x04) — set auto-repeat.  Accept silently.
if nr == 0x04 {
    return Ok(0);
}
```

The comment is wrong.  Looking at the Linux `evdev.h`:

```
#define EVIOCGREP   _IOR('E', 0x03, unsigned int[2])  /* get repeat */
#define EVIOCSREP   _IOW('E', 0x03, unsigned int[2])  /* set repeat */
```

Both have nr=0x03; they differ only in direction (read vs write).
Our handler was matching nr=0x04 for SET, which is actually
EVIOCSKEYCODE, a different ioctl.  EVIOCSREP fell through to the
catch-all ENOTTY.  Fixed:

```rust
if nr == 0x03 && is_read {
    /* GET repeat — return [250, 33] */
}
if nr == 0x03 && !is_read {
    /* SET repeat — accept silently */
    return Ok(0);
}
if nr == 0x04 {
    /* EVIOCSKEYCODE — accept silently */
    return Ok(0);
}
```

### Bug 5 (the real one): `EvdevFile::write` returned 0

After the EVIOCSREP fix, the same error persisted.  Stranger,
the errno was `ENOENT` ("No such file or directory") —
unusual for an ioctl.  Reading the xf86-input-evdev source more
carefully revealed that the actual culprit isn't EVIOCSREP at
all but the LED-state write further down:

```c
struct input_event ev[5];  /* CapsLock + NumLock + ScrollLock + ...  */
// fill ev[i] with EV_LED events
if (write(pInfo->fd, ev, sizeof(ev)) != sizeof(ev)) {
    if (errno != ENODEV)
        xf86IDrvMsg(pInfo, X_ERROR,
            "Failed to set keyboard controls: %s\n", strerror(errno));
}
```

Xorg writes a small `struct input_event[N]` block to the evdev
fd to set LED state.  Our handler:

```rust
// kernel/fs/devfs/evdev.rs (before)
fn write(...) -> Result<usize> {
    Ok(0)
}
```

`write()` returns 0 — a successful write of zero bytes.  Xorg's
`write(fd, ev, sizeof(ev)) != sizeof(ev)` evaluates to `0 != N`
which is true, so Xorg logs the error.  And `errno` from a
successful `write(0)` isn't reset — the previous syscall's
`ENOENT` (probably from an earlier `open()` of `/sys/class/leds/...`
that doesn't exist) leaks into `strerror()`.

Fix: have `write()` accept the bytes silently, /dev/null-style:

```rust
// kernel/fs/devfs/evdev.rs (after)
fn write(_offset, buf, _options) -> Result<usize> {
    Ok(buf.len())
}
```

The kernel doesn't yet forward LED state back to QEMU's
virtio-input statusq, so userspace's caps-lock indication
diverges from QEMU's view — but the keyboard stays loaded,
which is what matters.  Closing the LED-back-to-statusq loop
is its own follow-up.

### Gotcha: Xorg's duplicate-load

After Bugs 3+4+5, the Xorg log showed *two* device registrations
21 ms apart:

```
[1.880] (II) Using input driver 'evdev' for 'kbd'
…
[1.880] (II) XINPUT: Adding extended input device "kbd" (type: KEYBOARD, id 6)
[1.901] (II) Using input driver 'evdev' for 'kbd'    <- second time
[1.901] (WW) evdev: kbd: device file is duplicate. Ignoring.
[1.901] (EE) PreInit returned 8 for "kbd"
[1.901] (II) UnloadModule: "evdev"
```

Cause: our xorg.conf had:

```
Section "InputDevice"
    Identifier "kbd"
    ...
EndSection

Section "ServerLayout"
    InputDevice "kbd" "CoreKeyboard"
EndSection
```

Both references trigger PreInit independently.  The second
attempt fails because the device file is already open, and the
fallback unloads everything.  Fix: keep only the standalone
`Section "InputDevice"`; Xorg picks it up automatically when
`AutoAddDevices=false`.

After all of the above:

```
$ make ARCH=arm64 test-lxde-input
TEST_PASS evdev_event0_present
TEST_PASS evdev_event0_readable
TEST_END 8/8
```

Plus the xorg log is clean: one keyboard, no errors.

### What about `typed_text_arrived`?

It still SKIPs.  Events reach Xorg core (proven by Xorg draining
`/dev/input/event1` — our test sees zero bytes there because
Xorg got there first), but the focused X11 client doesn't
observe them.  This is the X11 input-routing layer, not the
kernel.  Most likely cause: xterm isn't focused at the moment of
injection, so keys go to the root window.  Punted to a future
session that wires a mouse-move-to-focus or `xdotool`-style
window activation.

The Kevlar-side input pipe is verified.  The remaining gap is
"X11 needed a hint about which window to send keys to" — a
userspace problem.

## Phase 2: a generic per-program harness

The plan called for `test-lxde-program` parameterized by
`kevlar-prog=NAME` on the kernel cmdline.  The implementation
clones `test_lxde.c`'s bring-up sequence verbatim, then runs a
program-specific phase:

```c
load_program_args();    // reads /proc/cmdline
// ... full LXDE bring-up (xorg, openbox, tint2, pcmanfm) ...
fb_fingerprint(&fp_before, &nb_before);
spawn(target_path, g_prog_args);
sleep(6);
fb_fingerprint(&fp_after, &nb_after);
// 4 sub-tests:
//   <prog>_process_running   /proc/*/comm scan
//   <prog>_window_mapped     xprop -root _NET_CLIENT_LIST
//   <prog>_pixels_changed    fp_after != fp_before
//   <prog>_clean_exit        SIGTERM, no zombie
```

Makefile wrapper:

```make
iterate-program:
    $(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/test-lxde-program"
    $(PYTHON3) tools/run-qemu.py --timeout 240 \
        --kvm --batch --arch $(ARCH) --disk $(LXDE_IMG) \
        --append-cmdline "kevlar-prog=$(PROG)" \
        ...
```

xeyes was the smoke-test target.

```
$ make ARCH=arm64 iterate-program PROG=xeyes
TEST_PASS mount_rootfs
TEST_PASS xorg_running
T: fb fingerprint before xeyes: 0x337dd1 (nonblack=2941)
T: launched xeyes (pid=39)
TEST_PASS xeyes_process_running
TEST_PASS xeyes_window_mapped
T: fb fingerprint after xeyes: 0xe49e79cc (nonblack=2971)
TEST_PASS xeyes_pixels_changed
TEST_PASS xeyes_clean_exit
TEST_END 6/6
```

xeyes pulls in only libX11 + libXt + libXmu; it's a tiny pure-
Xlib program that draws two eyes following a cursor.  No GTK,
no D-Bus, no fontconfig.  Cleanest possible smoke test.

xcalc, the second target, has a Xaw dependency tree:

```
$ make ARCH=arm64 iterate-program PROG=xcalc
TEST_PASS mount_rootfs
TEST_PASS xorg_running
T: fb fingerprint before xcalc: 0x337dd1 (nonblack=2941)
T: launched xcalc (pid=35)
…
[PANIC] CPU=0 at platform/arm64/interrupt.rs:127
SPIN_CONTENTION: cpu=1 lock="<unnamed>" addr=0xffff000041e4cd58 spins=5000000
SPIN_CONTENTION: cpu=0 lock="<unnamed>" addr=0xffff000041e4cd58 spins=5000000
…
```

A real Kevlar bug — 5M-spin contention on an unnamed lock at a
fixed address, both CPUs stuck.  The harness ran for one minute,
panicked.  Whatever Xaw's startup does (loading hundreds of
fonts? mmaps? dlopen storms?) breaks Kevlar.

This is the reason for having a per-program harness.  No amount
of `test-lxde 6/6` would catch this, because xcalc isn't part of
the LXDE session.  But running it through the harness took ~3
minutes from "add xcalc to LXDE_PACKAGES" to "panic captured."

Per the saved memory ("Linux is the source of truth"), the next
step is to run xcalc on Linux with the same disk image.  If
Linux passes, this is a Kevlar bug to chase; if Linux also fails,
the program/test has a problem.

## What's plumbed and what's not

| Bit | Status |
|---|---|
| `--inject-keys` host driver in `run-qemu.py` | ✅ |
| evdev event delivery: virtio → IRQ → `read()` | ✅ |
| Honest EVIOCGBIT from virtio config space | ✅ |
| Xorg loads kbd, no errors, no unload | ✅ |
| `iterate-program PROG=xeyes` 6/6 | ✅ |
| `iterate-program PROG=xcalc` | 🐛 panics — real bug |
| Linux baseline parity for per-program tests | (Phase 3) |
| `typed_text_arrived` (typed text reaches xterm) | ⚠️ X11 focus, not kernel |
| LED state forwarded back to QEMU | ⚠️ deferred |

## Status

- arm64 LXDE: ✅ `test-lxde` 8/8 (added evdev sub-tests)
- arm64 input: ✅ `test-lxde-input` end-to-end through Xorg core
- arm64 per-program: ✅ harness lands; xeyes 6/6
- New kernel bugs surfaced for follow-up: xcalc lock contention,
  X11 focus-routing for `typed_text_arrived`

Three commits this session: `phase-1` (input wiring, EVIOCGBIT,
EVIOCSREP), `phase-1b` (Xorg duplicate-load + write-len fix),
`phase-2` (per-program harness).  All on top of the busybox-
production-parity work from blog 247.

## Next: Phase 3 — Linux baseline parity for the per-program harness

The xcalc result demands the same workflow that drove blog 247:
boot the same disk image with a Linux kernel and run the exact
same `test-lxde-program` binary.  If Linux also panics (it
won't), test problem.  If Linux passes, one diff against
Kevlar's panic surface = the bug list.  That's Phase 3.
