# 272 — kABI K24: /dev/input/event2 = kabi-virtio-input

K24 lands.  Userspace can now `open("/dev/input/event2")`,
`ioctl(EVIOCGNAME)`, and find a device named `kabi-virtio-input`
— registered by **the Linux binary virtio_input.ko**, exposed
to userspace by **Kevlar's native Rust evdev implementation**,
**through a shared registry**.

```
kabi: virtio walk: probing driver 'virtio_input' against device_id=18
kabi: input_register_device: registered 'kabi-virtio-input' as
       /dev/input/event2 (total devices: 3)
kabi: virtio walk: 'virtio_input' probe returned 0

USERSPACE-INPUT: event0 name=QEMU Virtio Keyboard
USERSPACE-INPUT: event1 name=QEMU Virtio Mouse
USERSPACE-INPUT: event2 name=kabi-virtio-input
USERSPACE-INPUT: name=kabi-virtio-input
USERSPACE-INPUT: done
```

`make ARCH=arm64 test-userspace-input` is the new regression
target.  **25 kABI tests** pass.

## The shape that lands

Up until K24, kABI was a separate world from Kevlar's native
drivers.  /dev/dri/card0 came from a kABI-registered DRM
driver.  /dev/input/eventN came from Kevlar's *native* Rust
virtio_input crate.  Two parallel tracks for the same kind of
device, never touching.

K24 connects them.  Six lines in `exts/virtio_input/lib.rs`:

```rust
pub fn register_kabi_input_device(
    name: alloc::string::String,
) -> Arc<InputDevice> {
    let dev = InputDevice::new(name);
    INPUT_DEVICES.lock().push(dev.clone());
    dev
}
```

That's the whole bridge.  The native virtio_input driver
already pushes to `INPUT_DEVICES` at the end of its probe; K24
exposes the same path publicly so the kABI side can call it
without going through the rest of the driver-side machinery.

Then in `kernel/kabi/input.rs`:

```rust
pub extern "C" fn input_register_device(_dev: *mut c_void) -> i32 {
    let arc_dev = virtio_input::register_kabi_input_device(
        "kabi-virtio-input".into(),
    );
    let n = virtio_input::registered_devices().len();
    log::info!("kabi: input_register_device: registered '{}' as \
                /dev/input/event{} (total devices: {})",
                arc_dev.name.lock(), n - 1, n);
    0
}
```

That's it.  K23's virtio_input.ko probe calls
`input_register_device(idev)`; the function pushes a new
`InputDevice` to the shared registry; the existing
`EvdevFile(2).dev()` resolves to that entry on the next
read/poll/ioctl from userspace.

No new char device installation, no new fops adapter, no new
plumbing.  Kevlar's existing K-something evdev infrastructure
just *worked* — it had been waiting for someone to register
something.

## Three input devices coexist

The boot sequence in our QEMU setup creates three input
devices, two of them before kABI even gets a chance:

```
event0:  QEMU Virtio Keyboard       (native Rust virtio_input driver)
event1:  QEMU Virtio Mouse          (native Rust virtio_input driver)
event2:  kabi-virtio-input          (K24, via virtio_input.ko + kABI)
```

Same registry.  Same `EvdevFile`.  Same `/dev/input/event*`
char device tree.  Three different code paths arrived at the
same data structure and the same userspace-visible result.

This is what the LinuxKPI playbook is supposed to look like
when it lands.  You don't replace native code with binary
modules; you **let the binary module slot into the existing
infrastructure as just another producer**.  Kevlar's native
virtio_input still wakes up first because of timing — it's
called from boot init and is faster than the kABI module-load
+ probe path.  But there's nothing privileged about it.  The
kABI side gets the next free slot.

## What the userspace test actually verified

The test is 50 lines of C.  Open every `/dev/input/event{0..3}`
in turn, EVIOCGNAME on each:

```c
for (int i = 0; i < 4; i++) {
    snprintf(path, sizeof(path), "/dev/input/event%d", i);
    int fd = open(path, O_RDONLY | O_NONBLOCK);
    if (fd < 0) continue;
    char name[64] = {0};
    if (ioctl(fd, EVIOCGNAME(sizeof(name)), name) >= 0) {
        printf("USERSPACE-INPUT: event%d name=%s\n", i, name);
        if (strcmp(name, "kabi-virtio-input") == 0)
            printf("USERSPACE-INPUT: name=kabi-virtio-input\n");
    }
    close(fd);
}
```

Output proves:

1.  `event0..event2` all opened cleanly through Kevlar's VFS
    → tmpfs → EvdevFile dispatch.
2.  EVIOCGNAME returned a string for each, copied through
    Kevlar's user-buffer machinery.
3.  Among the strings was the one our kABI side wrote.

The first test where multi-source data arrives at the same
userspace endpoint and the right one shows up at the right
slot.

## What's not flowing yet

The fact that `event2` exists doesn't mean keystrokes work.
K24's `InputDevice::new("kabi-virtio-input")` creates a device
with empty `ev_bits` (no advertised key/relative/absolute
capabilities) and an empty event queue.  Userspace can:

- `open()` it ✓
- `EVIOCGNAME` it ✓
- `EVIOCGBIT(EV_KEY, ...)` and get back zero bits ✗ (well,
  technically returns "this device has no keys")
- `read()` it → blocks indefinitely (or with O_NONBLOCK,
  returns EAGAIN)
- `poll()` it → never becomes readable

To make event flow real, we'd need:

1.  An *event source*: something that pushes events into
    `INPUT_DEVICES[2].queue`.  The native driver gets this from
    real virtio MMIO interrupts.  The kABI side could either
    (a) inject events programmatically, (b) plumb interrupts
    through to the loaded virtio_input.ko's
    `virtinput_recv_events` callback, or (c) source events
    from QEMU's actual virtio-input device but via the kABI
    path.
2.  Capability advertisement: fill in `ev_bits[EV_KEY]` with
    a bitmap of supported keys so EVIOCGBIT returns useful
    answers.

K26+ wires real event flow.  K24 was just "the device exists."

## Why the bridge mattered

There was an alternative architecture: build a parallel
char-device tree (`/dev/input/kabi-event0`) for kABI-loaded
input drivers, separate from the native one.  That would have
been more "isolated" but also more confusing — userspace tools
that scan `/dev/input/event*` would miss kABI devices, and
xf86-input-evdev / libinput would need configuration to find
them.

The K24 design decision: **kABI drivers register through the
same path as native drivers.**  Same `InputDevice` struct.
Same `INPUT_DEVICES` registry.  Same `EvdevFile`.  Userspace
can't tell the difference.

That's the design pattern that makes the kABI arc work for
every other subsystem too.  K20's `drm_dev_register` is
already shaped this way — it installs `/dev/dri/cardN` through
Kevlar's K4 char-device tree, indistinguishable from native
DRM device registration.  K24 confirms the pattern
generalizes.

## Cumulative kABI surface (K1-K24)

~331 exports.  ~50 shim modules.  Six DRM modules loadable.
**Two probe-firing buses (PCI + virtio).  Two userspace-visible
endpoints created by kABI registration: `/dev/dri/card0` and
`/dev/input/event2`.**

## Status

| Surface | Status |
|---|---|
| K1-K23 | ✅ |
| K24 — input_register_device → /dev/input/event2 | ✅ |
| K25+ — DRM mode_config + KMS pipeline | ⏳ |

## What K25 looks like

The output side now needs the symmetric upgrade.  K20 made
`drm_dev_register` install /dev/dri/card0; K21 made
`DRM_IOCTL_VERSION` real.  But every modesetting userspace
(Xorg's modesetting driver, Mesa, kmscube) calls
`DRM_IOCTL_MODE_GETRESOURCES` immediately after VERSION/GET_CAP.

```
ioctl(fd, DRM_IOCTL_MODE_GETRESOURCES, &res):
    res.count_fbs       = 0..4
    res.count_crtcs     = 1..4
    res.count_connectors = 1..4
    res.count_encoders  = 1..4
    res.fb_id_ptr       → array of u32 fb IDs
    res.crtc_id_ptr     → array of u32 crtc IDs
    res.connector_id_ptr→ array of u32 connector IDs
    res.encoder_id_ptr  → array of u32 encoder IDs
    res.min/max_width/height = mode geometry bounds
```

K25's job is to make this return a sensible tuple — at least
one CRTC, one connector, one encoder — so userspace believes
there's something to drive.  That requires `drm_device.mode_config`
to be a real struct (not the zero-filled buffer K17 leaves
us with).  Probably the biggest single milestone since K17.

After K25:
- **K26**: real virtqueue event flow → keystrokes arrive at
  `/dev/input/event2`.
- **K27**: fbcon binding → kernel printk's appear on the
  framebuffer (first "pixels visible" milestone).
- **K28+**: Xorg starts.  Real graphical session.

The "graphical ASAP" arc is now **3 milestones away**.  K24
was the input-side bridge; K25 is the output-side.
