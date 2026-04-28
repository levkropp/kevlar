# 268 — kABI K19: the first probe firing

K19 lands.  For the first time in the kABI arc, **a real Ubuntu
kernel module's callback function runs inside Kevlar**.
cirrus_pci_probe traverses 11 calls into our kABI surface,
allocates a drm_device, ioremaps two BARs, and returns 0.

```
kabi: __pci_register_driver: name=Some("cirrus-qemu") mod=Some("cirrus_qemu")
kabi: cirrus init_module returned 0
kabi: __pci_register_driver: name=Some("bochs-drm") mod=Some("bochs")
kabi: bochs init_module returned 0
kabi: registered fake PCI device 0x1013:0x00B8 (Cirrus VGA), pdev_buf=0xffff…
kabi: PCI walk: 2 driver(s), 1 device(s)
kabi: PCI walk: probing driver 'cirrus-qemu' against 1013:00b8
kabi: __devm_drm_dev_alloc: size=7040 offset=0 buf=0xffff… drm_dev=0xffff…
kabi: PCI walk: 'cirrus-qemu' probe returned 0
```

`make ARCH=arm64 test-module-k19` is the new regression target.
**20 kABI tests** now pass.  K19 is the inflection point: every
prior milestone was "load `.ko`, satisfy linker, no callbacks
fire."  K19 fires one.

## How it works

K17 left `__pci_register_driver` as a one-liner — log + return 0.
K19 turns it into a real bus implementation:

1. **Driver registry**: `SpinLock<Vec<RegisteredDriver>>` records
   each driver's `(struct pci_driver *, name)` pair when
   `__pci_register_driver` is called.
2. **Static fake PCI device**: `kernel/kabi/pci.rs` declares a
   single fake Cirrus VGA device — vendor=0x1013, device=0x00B8 —
   backed by a 4KB buffer with `resource[0]` and `resource[2]`
   populated at offsets 1024/1032/1088/1096 (verified by
   disassembling cirrus_pci_probe's BAR reads).
3. **Bus walker**: after all modules load, `kabi::pci::walk_and_probe()`
   iterates registered drivers, walks each driver's `id_table` (40-byte
   pci_device_id entries, sentinel `vendor=0`), matches against
   fake devices, and invokes `probe(pdev, matched_id)`.

The matching logic is 30 lines.  Linux's actual PCI bus driver
is thousands.  Same pattern as K11's `__register_virtio_driver`
upgrade trajectory — start with "log + 0," upgrade when probe
needs to fire.

## The layout assumptions that held

Three offsets needed to be right for K19:

```
struct pci_driver:
  +0  name (const char *)
  +8  id_table (struct pci_device_id *)
  +16 probe (function pointer)

struct pci_device_id (40 bytes):
  +0  vendor (u32)
  +4  device (u32)
  ...
  vendor=0 → end of table

struct pci_dev (the fake one):
  +0xd0  embedded struct device     ← passed as parent to alloc
  +1024  resource[0].start          ← passed to devm_ioremap
  +1032  resource[0].end
  +1088  resource[2].start
  +1096  resource[2].end
```

I verified these offsets by `objdump -dr` on cirrus-qemu.ko's
`.data` section before writing any code.  The relocations on
`cirrus_pci_driver` literally pointed at offset 0/8/16 with
labels `.rodata+...` for name, `.rodata+0x548` for id_table,
and `cirrus_pci_probe` for the function.  Disassembly of probe
confirmed the resource[]/dev offsets.  No surprises.

## The shape of the cascade — and why there wasn't one

The K19 plan baked in cascade-risk language: "stubs returning 0
that need to return non-null... layout offset drift...
iteration-cascade-prone."  None of that happened.  Here's
why.

cirrus_pci_probe traverses:

```
aperture_remove_conflicting_pci_devices(pdev, "cirrus-qemu")  →  0  ✓
pcim_enable_device(pdev)                                      →  0  ✓
pcim_request_all_regions(pdev, "cirrus-qemu")                 →  0  ✓
cirrus = __devm_drm_dev_alloc(&pdev->dev, &driver, 7040, 0)
                                                              →  real 7040-byte buf
devm_ioremap(0x1_0000_0000, 4096)                             →  fake VRAM VA
devm_ioremap(0x1_1000_0000, 4096)                             →  fake mmio VA
drmm_mode_config_init(cirrus)                                 →  0  ✓
*(cirrus + 808/816/824/1440) = ...                            →  writes into our buffer
drm_universal_plane_init(cirrus, ...)                         →  0  ✓
drm_plane_enable_fb_damage_clips(cirrus->plane)               →  no-op  ✓
drm_mode_config_reset(cirrus)                                 →  no-op  ✓
drm_dev_register(cirrus, 0)                                   →  0  ✓
return 0
```

Three K17 stubs needed to upgrade — `__devm_drm_dev_alloc`,
`devm_ioremap`, `devm_ioremap_wc`.  The other 8 stayed as
"return 0 / no-op."  Why?

Because **K17's stub design was right**.  The functions that
*write* into structs we control (drm_dev's mode_config) just
write into our zero-initialized buffer — those writes are
inert.  The functions that allocate (drm_managed-style) just
needed real backing memory — `kzalloc`-shaped.  The functions
that report success (`drm_dev_register`) need only return 0
because nothing downstream reads the registration state at K19.

The cascade *will* surface in K20+ when:
- Userspace tries to `open("/dev/dri/card0")` — `drm_dev_register`
  needs to expose a real char device.
- Probe code reads back what it wrote earlier — currently we
  only write, never read.
- Multiple drivers share a fake device — `dev_set_drvdata`
  reads need to round-trip.

But for "probe runs to completion and returns 0" — K17's stubs
were perfectly shaped.  The Linuxulator playbook works.

## The one iteration

The first `make test-module-k19` run panicked in
`buddy_alloc.rs:157` — index out of bounds on `free_lists[12]`.
Cause: the fake VRAM was 16MB (=2^12 pages), exceeding the
buddy allocator's max order of 11 (8MB).

Fix: shrink BAR0 to 4KB.  cirrus probe doesn't actually use
the buffer — it just stashes the ioremap'd pointer in
`drm_dev`.  4KB is plenty.

That was the entire iteration cost of K19.  Build → boot →
read panic → 5-line edit → rebuild → green.

## What this means

The kABI arc has two halves: "load modules" (K1-K18) and "run
modules" (K19+).  K19 is the bridge.  Until today, every
Ubuntu module Kevlar loaded was inert — the linker resolved
its symbols, but no code ever ran outside `init_module`.

Now cirrus_pci_probe ran.  Real Linux code from
`drivers/gpu/drm/tiny/cirrus-qemu.c`, compiled by Ubuntu's
build system, linked against Kevlar's kABI exports, called by
Kevlar's PCI walker, allocating buffers from Kevlar's slab,
registering callbacks Kevlar will eventually invoke.

We are running an Ubuntu kernel driver in a non-Linux kernel.

## What's still open

bochs is registered as a PCI driver but no fake bochs device
exists.  Trivial to add (same shape, different vendor/device).

`drm_dev_register` returns 0 but creates no `/dev/dri/cardN`.
Userspace can't yet open the device.  K20+ wires the char
device.

The `dev_set_drvdata`/`dev_get_drvdata` round-trip currently
goes through `struct device`'s K3 layout (which differs from
Linux's).  When K22+ tries to retrieve drvdata to find the
drm_device from inside an ioctl handler, layout exactness
gets enforced.

cirrus's `cirrus_pci_remove`, `cirrus_pci_shutdown` haven't run.
They will only matter if/when we drop the module, which is
post-K30.

## Cumulative kABI surface (K1-K19)

~331 exports.  ~50 shim modules.  Six DRM-related modules
loadable: drm_buddy, drm_exec, drm_ttm_helper, drm_dma_helper,
cirrus-qemu, bochs.  One probe firing successfully.

## Status

| Surface | Status |
|---|---|
| K1-K18 | ✅ |
| K19 — PCI bus walking + cirrus probe firing | ✅ |
| K20+ — bochs probe + /dev/dri/card0 + virtio probe | ⏳ |

## What K20 looks like

K20 has options.  In rough order of escalating payoff:

1. **Fire bochs probe too.**  Add a fake bochs PCI device
   (vendor=0x1234, device=0x1111) — 5-line change to `pci.rs`.
   Two probe-firing drivers instead of one.  Cheap session.
2. **Real `drm_dev_register`** — expose `/dev/dri/card0` as a
   char device.  Userspace can `open()` and `ioctl()` (most
   ioctls return -ENOSYS for now).  Bridge to fbcon /
   modesetting.
3. **virtio bus walking** — same pattern as K19 but for the
   K12 virtio_input.ko driver.  Probe fires, input device
   gets registered, `/dev/input/event0` becomes a real path.
4. **fbcon-on-Kevlar** — Linux's fbcon connects to a
   `drm_device` via `drm_fbdev_*` paths.  Once /dev/dri/card0
   is real, fbcon attempts to render text; sys_copyarea /
   sys_fillrect / sys_imageblit (K15 stubs) get exercised.

(1) is trivial; (2)-(3) are medium; (4) is the visible payoff.
The "graphical ASAP" arc is now **5 milestones** away, and K19
is where the curve flattens — every subsequent milestone moves
real bytes between userspace and a Linux kernel driver running
inside a Rust microkernel.
