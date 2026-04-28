# 267 — kABI K18: bochs.ko, the 83% compounding ratio

K18 lands.  Ubuntu 26.04's `bochs.ko` — Linux's KMS driver for
QEMU's Bochs Display Adapter — loads in Kevlar with all 107
symbols resolved and `init_module` returns 0.

```
kabi: loading /lib/modules/bochs.ko (Ubuntu 26.04)
kabi: loaded /lib/modules/bochs.ko (47569 bytes, 46 sections, 223 symbols)
kabi: /lib/modules/bochs.ko license=Some("GPL")
       author=Some("Gerd Hoffmann <kraxel@redhat.com>")
       desc=Some("DRM Support for bochs dispi vga interface (qemu stdvga)")
kabi: applied 415 relocations (41 trampoline(s))
kabi: bochs init_module returned 0
```

`make ARCH=arm64 test-module-k18` is the new regression target.
**19 kABI tests** now pass.

## The compounding payoff hits a new high

K18 was the smallest milestone in three sessions, and the proof
the kABI runtime has reached escape velocity.  bochs.ko is the
**second real DRM driver** Kevlar loads (after K17's
cirrus-qemu).  It has 107 undefined symbols — comparable to
cirrus's 88 — but **89 of them were already stubbed**.

```
total undefs:      107
already stubbed:    89   (drm_dev / drm_kms / pci / drm_format /
                          drm_atomic / fb / mutex / refcount / ...)
net new:            18   ← 83% compounding ratio
```

The K-by-K ratio of net-new undefs to total:

| K | Module | Total | Net new | Compounded |
|---|---|---|---|---|
| K11 | dummy.ko (network) | 23 | 23 | 0% |
| K12 | virtio_input.ko | 30 | 30 | 0% |
| K13 | drm_buddy.ko | 21 | 13 | 38% |
| K14 | drm_exec.ko | 11 | 9 | 18% |
| K15 | drm_ttm_helper.ko | 47 | 40 | 15% |
| K16 | drm_dma_helper.ko | 79 | 32 | 60% |
| K17 | cirrus-qemu.ko | 88 | 81 | 8% |
| **K18** | **bochs.ko** | **107** | **18** | **83%** |

K17 was a step backward in compounding because it brought a
*new class* of stubs (drm.ko-equivalent surface).  K18 was the
test of whether that class amortizes — and the answer is yes,
emphatically.  Two PCI-driven KMS drivers share most of their
surface; once the first one's stubs exist, the second is
trivial.

## bochs.ko's 18 net-new symbols

```
DRM EDID  (5):                     DRM core extensions  (5):
  drm_edid_connector_add_modes       drm_dev_put
  drm_edid_connector_update          drm_crtc_vblank_off
  drm_edid_free                      drm_connector_attach_edid_property
  drm_edid_header_is_valid           drm_mode_config_helper_resume
  drm_edid_read_custom               drm_mode_config_helper_suspend

PCI / IO resources  (4):           Port I/O extensions  (3):
  __devm_request_region              logic_inb
  devm_ioremap_wc                    logic_inw
  iomem_resource (static)            logic_outw
  ioport_resource (static)

DRM debug  (1):
  __drm_err
```

One new file (`kernel/kabi/drm_edid.rs`).  Five extensions.  All
stubs no-op since probe doesn't fire.

## What lights up under the hood

This is the moment the LinuxKPI pattern shines.  bochs.ko's
`init_module` does *nothing different* from cirrus-qemu's:

```c
static int __init bochs_init(void) {
    if (drm_firmware_drivers_only() && bochs_modeset == -1)
        return -EINVAL;
    if (bochs_modeset == 0)
        return -EINVAL;
    return pci_register_driver(&bochs_pci_driver);
}
```

It calls `video_firmware_drivers_only` (already stubbed K17,
returns false), reads a module-param flag, then calls
`__pci_register_driver` (already stubbed K17, logs + returns 0).
Done.  Init returns 0.

The whole "second driver" gets us **for free** by virtue of
sharing the K17 surface.  The 18 net-new symbols are
references from probe / release / runtime callbacks that
will fire when probe actually runs — and like K17, that's K19.

## The new-stub flavor: EDID

The most interesting K18 sub-surface is **EDID** — Extended
Display Identification Data, the byte sequence a monitor
returns over DDC/HDMI to identify itself (resolutions, vendor,
serial).  bochs reads a synthetic EDID from the QEMU
bochs-display PCI BAR.

Real Linux EDID parsing is hundreds of lines: header
validation, checksum, parsing of standard timing blocks,
detailed timing descriptors, CEA extension blocks.

K18 EDID stubs:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn drm_edid_connector_add_modes(
    _connector: *mut c_void,
) -> i32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn drm_edid_header_is_valid(_buf: *const c_void) -> i32 { 0 }
```

Five lines per function.  When K22+ a real fbcon surface needs
real display modes, this file gets a real parser — pretty
trivially (the EDID blob is 128 bytes; the fields we care
about for modes are well-known).

## What didn't have to be done

- **PCI bus walking.**  Same as K17 — `__pci_register_driver`
  records nothing.
- **Real `iomem_resource` / `ioport_resource` tree.**  Linux
  uses these to track IO regions.  Ours are 64-byte zeroed
  buffers.  K20+ when probe paths actually request regions.
- **Real EDID parsing.**  K22+.

## Cumulative kABI surface (K1-K18)

~331 exported symbols.  ~50+ shim modules in `kernel/kabi/`.
**Six Ubuntu kernel modules now loadable**:

- drm_buddy.ko (DRM buddy allocator)
- drm_exec.ko (DRM exec context)
- drm_ttm_helper.ko (DRM TTM helpers)
- drm_dma_helper.ko (DRM DMA helpers)
- cirrus-qemu.ko (Cirrus VGA KMS)
- **bochs.ko** (Bochs Display KMS)

Plus dummy.ko (network), virtio_input.ko, xor-neon.ko,
bman-test.ko, and the K1-K8 internal hello-world corpus.

## Status

| Surface | Status |
|---|---|
| K1-K17 | ✅ |
| K18 — bochs.ko + EDID/IO stubs | ✅ |
| K19 — PCI bus walking + cirrus probe firing | ⏳ |

## What K19 looks like

K19 is the **first probe-firing milestone of the kABI arc**.

Right now `__pci_register_driver` is a no-op log + return 0.
K19 turns it into:

1. **Track registered drivers** in a list (driver_list, like
   Linux's `pci_drivers`).
2. **Statically declare a fake PCI device** in
   `kernel/kabi/pci.rs` — vendor=0x1013, device=0x00B8 (Cirrus
   CL-GD5446).  Later (K23+?) sourced from real QEMU PCI
   enumeration.
3. **At register time, walk the fake device list, match against
   the driver's `id_table`, and call `probe()`.**

That sounds simple.  Then `cirrus_pci_probe()` runs and starts
calling our K17 stubs — `pcim_enable_device` (returns 0,
fine), `__devm_drm_dev_alloc` (returns null... oops).

Cascade begins.  K17's stubs were "satisfy the linker"; K19
turns each into "actually return something the caller can
work with."

The shape of the cascade:

```
__pci_register_driver
  → walk fake devices
  → match id_table
  → cirrus_pci_probe(pdev, id)
      → pcim_enable_device(pdev)         [K17 stub returns 0 — OK]
      → pcim_request_all_regions(pdev)   [K17 stub returns 0 — OK]
      → aperture_remove_conflicting...   [K17 stub returns 0 — OK]
      → cirrus = devm_drm_dev_alloc(...) [K17 stub returns null — CRASH]
```

So K19 starts there: make `__devm_drm_dev_alloc` allocate a
real buffer.  Linux's signature is

```c
struct drm_device *__devm_drm_dev_alloc(
    struct device *parent,
    const struct drm_driver *driver,
    size_t size, size_t offset);
```

where the caller's wrapping struct has a `struct drm_device`
embedded at `offset` within `size` bytes.  Real implementation
allocates `size` bytes, returns `buffer + offset`.

Then probe reads fields — `dev->dev`, `dev->driver`, etc. —
and we discover what struct layouts have to match Linux.  That
exercise is K19's bulk: **layout exactness** for `struct
drm_device`, `struct pci_dev`, `struct pci_driver`, `struct
pci_device_id`.

The "graphical ASAP" arc is now **5 milestones away** but K19
is the inflection where the arc starts looking like Linux work
again rather than stub generation.  Iteration cost goes up;
per-milestone payoff goes up too.
