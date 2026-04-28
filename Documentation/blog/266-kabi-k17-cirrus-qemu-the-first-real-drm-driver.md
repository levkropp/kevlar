# 266 — kABI K17: cirrus-qemu.ko, the first real DRM driver

K17 lands.  Ubuntu 26.04's `cirrus-qemu.ko` — Linux's KMS driver
for QEMU's emulated Cirrus VGA — loads in Kevlar with all 88
symbols resolved and `init_module` returns 0.

```
kabi: loading /lib/modules/cirrus-qemu.ko (Ubuntu 26.04)
kabi: loaded /lib/modules/cirrus-qemu.ko (50449 bytes, 45 sections, 174 symbols)
kabi: /lib/modules/cirrus-qemu.ko license=Some("GPL")
       desc=Some("Cirrus driver for QEMU emulated device")
kabi: applied 666 relocations (32 trampoline(s))
kabi: __pci_register_driver (stub)
kabi: cirrus init_module returned 0
```

`make ARCH=arm64 test-module-k17` is the new regression target.
18 kABI tests now pass.  K17 was the inflection point of the
kABI arc — the first time Kevlar loaded a *real* DRM driver,
not a helper library.

## What "real driver" means

K13-K16 loaded four DRM **helper** modules: drm_buddy, drm_exec,
drm_ttm_helper, drm_dma_helper.  Helpers don't run at load time
— they're pure library code consumed by drivers.  Their
`init_module` either does nothing (drm_exec, drm_ttm_helper,
drm_dma_helper are pure libraries) or does one tiny thing
(drm_buddy creates a slab cache).

cirrus-qemu is different:

```c
static int __init cirrus_init(void) {
    return pci_register_driver(&cirrus_pci_driver);
}
```

It registers a PCI driver.  Linux's *real* `pci_register_driver`
records the driver in a list, then walks pending PCI devices
and calls `probe()` on each match.  Our stub just records and
returns 0 — the same shape K12's `__register_virtio_driver`
took for virtio_input:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn __pci_register_driver(
    _drv: *mut c_void, _owner: *mut c_void, _mod_name: *const c_char,
) -> i32 {
    log::info!("kabi: __pci_register_driver (stub)");
    0
}
```

So `init_module` returns 0 having registered a driver that
hasn't been called for.  The other 80 net-new symbols (probe
callbacks, KMS atomic helpers, GEM shadow plane state, etc.)
sit in memory awaiting real PCI bus walking.  That's K18+.

## The biggest single batch yet

K17 was the largest milestone of the kABI arc by stub count:

```
total undefs:      88
already stubbed:   7
net new:           81

new shim files (5):
  drm_dev.rs         — 12 funcs (drm_dev_register, drm_open, ...)
  drm_kms.rs         — 31 funcs (atomic helpers + CRTC/encoder/
                        connector/plane/vblank objects)
  drm_gem_shadow.rs  — 11 funcs (shadow planes + GEM shmem +
                        fbdev_shmem probe)
  pci.rs             — 7 funcs (__pci_register_driver, devm_ioremap,
                        aperture_remove_conflicting_pci_devices,
                        video_firmware_drivers_only, …)
  tracepoints.rs     — 8 entries (4 mmio tracepoint statics +
                        4 log_*_mmio no-op functions)

extensions (4):
  drm.rs             — 9 helper funcs (drm_format_info,
                        drm_fb_clip_offset, drm_set_preferred_mode, …)
  printk.rs          — _dev_warn (variadic)
  fops.rs            — noop_llseek
  io.rs              — logic_outb (no-op on aarch64)
```

Double K15's 40 stubs.  But still **one session, one
iteration**.

## The single iteration

The first build run surfaced one missed undef:

```
kabi: undefined external symbol 'video_firmware_drivers_only'
       — not in kernel exports table
kabi: cirrus-qemu load_module failed: ENOENT
```

`video_firmware_drivers_only` is a Linux predicate gated by the
`video=firmware` kernel cmdline — checks whether to load only
firmware-provided framebuffer drivers.  Five-line fix in
`pci.rs`:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn video_firmware_drivers_only() -> bool {
    false
}
ksym!(video_firmware_drivers_only);
```

Rebuild, retry, init_module returns 0.  This is what
"compounding payoff" looks like at full strength: 81 net-new
stubs and only one missed undef out of 88, fixed in seconds
because the loader logs missing symbols by **name**.

## 666 relocations + 32 trampolines

cirrus-qemu's load applied 666 relocations — ~3× drm_dma_helper's
387, ~9× drm_buddy's 258.  32 R_AARCH64_CALL26 relocations
needed trampolines (they exceeded the ±128MB direct-call range
between module memory and the kABI export table).

Most of those 666 are `R_AARCH64_ABS64` and
`R_AARCH64_ADR_PREL_PG_HI21` + `R_AARCH64_ADD_ABS_LO12_NC` pairs
— the standard "load address of static data into a register"
pattern.  cirrus-qemu has many static structs (the DRM driver
function-table, the PCI ID table, the KMS callback tables) and
each reference relocates to its section base + offset.

The relocator's K1-K11 design (~150 LOC across `arch/arm64.rs`)
hasn't needed any new relocation-type handling since K11.  Every
new module's load just exercises the same handful of
R_AARCH64_* kinds at increasing volume.

## What didn't have to be done

- **PCI bus walking.**  `__pci_register_driver` records nothing,
  walks nothing.  K18+.
- **cirrus_pci_probe firing.**  Without bus walking, never
  invoked.
- **Real `__devm_drm_dev_alloc`.**  Returns null at K17.  Probe
  would crash on the next line — but probe doesn't run.
- **`/dev/dri/card0` exposed to userspace.**  K20+ when probe
  runs *and* registers a char device.
- **Real KMS atomic commits.**  All atomic helpers no-op.

## What this means for the "graphical ASAP" arc

Two roads forward, one short, one shorter:

| Path | Description |
|---|---|
| **K18 = bochs.ko** | A second small DRM driver (107 undefs, much overlap with K17), validates the K17 surface generalizes.  Predictable session, low cascade risk. |
| **K18 = PCI bus walking + cirrus probe** | The harder direct path.  Implement real `__pci_register_driver` (track drivers + walk fake PCI devices + call probe).  When probe runs, layout exactness work begins immediately and cascades.  Higher payoff per milestone, higher risk. |

K17's blog leaves the K18 framing open.  Whichever the next
plan picks, the trajectory stays the same:
- ~5 milestones to graphical Alpine.
- Probe firing (somewhere in K18-K19) is the bridge between
  "loaded modules" and "running drivers."
- Real `/dev/dri/card0` likely K20-K21.
- Xorg + virtio_input + cirrus pixels: K22-K23.

## Cumulative kABI surface (K1-K17)

~313 exported symbols (K16's ~232 + 81 K17 net new).  50+ shim
modules in `kernel/kabi/`.  Five Ubuntu kernel modules now
loadable: drm_buddy, drm_exec, drm_ttm_helper, drm_dma_helper,
**cirrus-qemu**.  Plus dummy.ko (network), virtio_input.ko
(input + virtio bus), xor-neon.ko (NEON XOR accel),
bman-test.ko, and the K1-K8 internal hello-world test corpus.

## Status

| Surface | Status |
|---|---|
| K1-K16 | ✅ |
| K17 — cirrus-qemu.ko + 81 DRM core/KMS/PCI stubs | ✅ |
| K18+ — second driver or first probe firing | ⏳ |

K17 was the milestone where Kevlar started looking like a
**Linux-binary-compatible kernel** rather than a Rust microkernel
that happens to load a few `.ko` files.  The next milestone is
where one of those `.ko` files starts running.
