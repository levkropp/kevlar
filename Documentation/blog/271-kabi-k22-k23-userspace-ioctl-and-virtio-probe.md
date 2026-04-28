# 271 — kABI K22 + K23: userspace ioctl + virtio probe firing

Two milestones, one post.  K22 was small enough to land in
minutes; K23 followed the K19 playbook for a different bus.
Together they validate that the kABI layers Kevlar built up
work end-to-end from real userspace and across the virtio bus.

## K22: a real userspace process talked to /dev/dri/card0

K21 made `drm_ioctl()` real.  K22 added the missing piece:
**proof that real userspace can use it**.

```
USERSPACE-DRM: starting
USERSPACE-DRM: open ok
USERSPACE-DRM: name=kabi-drm version=2.0.0
USERSPACE-DRM: done
PID 1 exiting with status 0
```

The test program is 50 lines of C.  It opens /dev/dri/card0,
calls `ioctl(fd, DRM_IOCTL_VERSION, &v)`, prints what comes
back, exits.  When run as PID 1 in Alpine userspace, the
output gets captured to serial and grep'd.

```c
struct drm_version v;
char name_buf[64], date_buf[64], desc_buf[64];
v.name = name_buf;  v.name_len = sizeof(name_buf);
v.date = date_buf;  v.date_len = sizeof(date_buf);
v.desc = desc_buf;  v.desc_len = sizeof(desc_buf);

if (ioctl(fd, DRM_IOCTL_VERSION, &v) < 0)
    return 2;

printf("USERSPACE-DRM: name=%.*s version=%d.%d.%d\n",
       (int)v.name_len, name_buf,
       v.version_major, v.version_minor, v.version_patchlevel);
```

What this exercise actually validates is the entire pipe:
**glibc's ioctl(2) → arm64 syscall trap → Kevlar's
sys_ioctl → VFS dispatch → tmpfs lookup → KabiCharDevFile →
DRM_FOPS_ADAPTER → drm_ioctl_adapter → drm_ioctl(VERSION) →
copy struct → drm_ioctl_version → write strings + version →
return 0 → reverse the path back to userspace**.

Eight layers, all built incrementally K1-K21.  None of them
broke when a real Alpine binary used them.  That's the
"satisfying click" of the LinuxKPI playbook arriving — every
module Kevlar loaded, every shim Kevlar wrote, every offset
Kevlar guessed at, all surface a single API call that does
exactly what userspace expects.

K22 was 30 minutes of typing.  Most of the time was waiting
for the kernel to compile.

## K23: virtio_input.probe() ran inside Kevlar

K19 made cirrus's PCI probe fire.  K23 was the same exercise
for the virtio bus and virtio_input.ko.

```
kabi: __register_virtio_driver: name=Some("virtio_input")
kabi: virtio_input init_module returned 0
kabi: registered fake virtio device device_id=18 vendor=0xffffffff
kabi: virtio walk: 1 driver(s), 1 device(s)
kabi: virtio walk: probing driver 'virtio_input' against device_id=18
kabi: virtio walk: 'virtio_input' probe returned 0
```

The PCI walker we built for K19 ports almost directly to
virtio.  Same shape:

```rust
struct RegisteredVirtioDriver { drv: usize, name_ptr: *const c_char }
static REGISTERED: SpinLock<Vec<RegisteredVirtioDriver>>
    = SpinLock::new(Vec::new());

#[unsafe(no_mangle)]
pub extern "C" fn __register_virtio_driver(drv, _owner) -> i32 {
    REGISTERED.lock().push(...);
    0
}

pub fn walk_and_probe() {
    for driver in REGISTERED { for device in fake_devices {
        if id_match(driver, device) { probe(device); }
    }}
}
```

Different layout offsets (id_table @ +152, probe @ +200 in
struct virtio_driver vs +8/+16 in struct pci_driver).  Different
struct virtio_device_id (8 bytes vs PCI's 40).  But the
mechanism is identical.

## The trickier part: vdev->config function-pointer indirection

The wrinkle compared to PCI: virtio drivers don't access bus
fields directly.  They go through a vtable —
`vdev->config->find_vqs(...)` — where `config` is a pointer
loaded at offset 832 of the virtio_device, and `find_vqs` is
a function pointer at offset 48 inside that vtable.

PCI's resource[] reads were direct:
```asm
ldr x0, [x22, #1024]     ; pdev->resource[0].start
```

virtio's vtable reads are two-step:
```asm
ldr x1, [x0, #832]       ; cfg = vdev->config
ldr x5, [x1, #48]        ; find_vqs = cfg->find_vqs
blr x5                   ; call find_vqs(...)
```

Which means our fake virtio_device needed not only a populated
buffer but a populated *vtable too*.  The K23 implementation
allocates two heap regions:

```rust
// 256-byte vtable buffer.  Each slot is a function pointer.
let cfg_buf = kzalloc(256, 0);
write(cfg_buf + CFG_OFF_FIND_VQS, fake_find_vqs as usize);
write(cfg_buf + CFG_OFF_GET_FEATURES, fake_get_features as usize);
// ... 11 entries total

// 2KB vdev buffer.
let vdev_buf = kzalloc(2048, 0);
write(vdev_buf + VDEV_OFF_CONFIG, cfg_buf);  // vtable ptr
write(vdev_buf + VDEV_OFF_FEATURES, VIRTIO_F_VERSION_1);
```

Eleven `extern "C" fn` Rust stubs.  Each takes the same first
arg (`*mut virtio_device`) and returns 0 / null / void / true /
the right "no-op" answer.  When the driver calls
`vdev->config->generation()`, control transfers from
virtio_input.ko's compiled-in code into our Rust stub, runs,
returns.  Across an ABI boundary that's been part of Linux's
internal kernel design for 17 years.

## What virtio_input.probe actually did

Tracing the disasm + the boot log, virtinput_probe ran
through:

1. Read `vdev->features` at offset 872. Got VIRTIO_F_VERSION_1
   (bit 32 set). Pass.
2. `__kmalloc_cache_noprof(3520, ...)` → 3520-byte vinput struct.
3. Store vdev pointer in vinput->vdev (offset 0).
4. Store vinput pointer in vdev->priv (offset 888).
5. **Call virtinput_init_vqs(vinput)**: this is where
   indirect calls into `cfg->find_vqs(vdev, 2, vqs[], ...)`
   happen. Our `fake_find_vqs` populated vqs[0..1] with
   pointers to two 4KB zero buffers (the fake event_vq and
   status_vq) and returned 0.
6. **Call input_allocate_device** (K12 stub, returns 2KB buffer).
7. Loop through virtinput_cfg_select calls — multiple
   `cfg->generation()`, `cfg->get_status()`, `cfg->set()`,
   `cfg->get()` invocations, all going through our fake
   config_ops vtable, all returning 0 / no-op.
8. **Call input_register_device** (K12 stub, returns 0).
9. Return 0.

About 25 indirect calls between the `blr x5` for find_vqs and
the final return.  None of them needed any K12 stub upgraded.
The full virtio_input bring-up sequence ran inside Kevlar
without a single iteration cost.

## What's *actually* registered

There's a subtlety worth calling out.  When virtio_input.probe
returns 0, Linux semantically considers the input device
"registered" — keystrokes from the underlying virtqueue should
flow into `/dev/input/eventN`.

In Kevlar today, none of that flows.  K12's
`input_register_device` is a no-op stub — nothing creates a
char device, nothing wires interrupts, nothing pushes events.
The driver thinks it's bound.  Userspace has nothing to open.

K24+ is where this gets fixed: real `input_register_device`
that exposes `/dev/input/event0`, then real virtqueue
interrupts (eventually) so keystrokes arrive.  For now, the
"probe fired and returned 0" milestone is what K23 delivers.

## The compounding ratio across buses

Every probe-firing milestone teaches us how Linux's drivers
*actually* lay out their function-pointer tables.  PCI's
ABI: id_table at +8, probe at +16.  Virtio's: id_table at
+152, probe at +200.  Verified once via objdump, hardcoded
in our walker, never revisited.

The "Linuxulator playbook" — start with stubs, upgrade only
when probes need it — is now exercised across two bus types
and two real Ubuntu drivers (cirrus-qemu, virtio_input),
both of which complete probe successfully.  The pattern
generalizes.  bochs probe returned -ENODEV at K20 because of
a hardware-ID check, but the *infrastructure* worked the same.

## Cumulative kABI surface (K1-K23)

~331 exports.  ~50 shim modules.  Six DRM modules loadable.
**Two real driver probes complete (cirrus, virtio_input).
One probe fires-but-fails (bochs).  Userspace ioctl path
verified by an Alpine binary running as PID 1.**

## Status

| Surface | Status |
|---|---|
| K1-K21 | ✅ |
| K22 — userspace DRM ioctl test | ✅ |
| K23 — virtio bus walking + virtio_input probe | ✅ |
| K24+ — /dev/input/event0 + DRM modesetting | ⏳ |

## What K24 looks like

Three threads, in escalating order:

1. **Real `input_register_device` → /dev/input/event0**.
   K12's stub creates nothing; K23's probe calls it and gets
   nothing back.  K24 wires it through Kevlar's K4 char-device
   tree (similar to how K20 wired drm_dev_register).
   Userspace can open `/dev/input/event0` even if no events
   flow.
2. **DRM_IOCTL_MODE_GETRESOURCES**.  The next ioctl after
   DRM_IOCTL_VERSION that any modesetting userspace
   (drmModeGetResources, libdrm tools) will call.  Returns
   arrays of CRTC / encoder / connector IDs.  Requires real
   drm_device.mode_config.
3. **fbcon binding to /dev/dri/card0**.  Linux's fbcon hooks
   into a DRM device's modesetting and writes the kernel
   console there.  When this works, kernel printk's appear
   on the framebuffer.  First "pixels visible" milestone.

The "graphical ASAP" arc is now **3 milestones away**.  K23
was the input-side bridge; K24 is where userspace stops being
the test harness and starts being the driver.
