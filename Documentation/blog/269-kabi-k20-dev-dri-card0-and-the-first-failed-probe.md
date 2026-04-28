# 269 — kABI K20: /dev/dri/card0 and the first failed probe

K20 lands.  Userspace can now `ls /dev/dri/card0` and see a
real char device, registered by Kevlar's PCI walker invoking
cirrus-qemu's probe and walking it through Linux's
`drm_dev_register` path into Kevlar's K4 char-device tree.

```
kabi: PCI walk: 2 driver(s), 2 device(s)
kabi: PCI walk: probing driver 'cirrus-qemu' against 1013:00b8
kabi: __devm_drm_dev_alloc: size=7040 offset=0 buf=… drm_dev=…
kabi: drm_dev_register: /dev/dri/card0 installed (major=226, minor=0)
kabi: PCI walk: 'cirrus-qemu' probe returned 0
kabi: PCI walk: probing driver 'bochs-drm' against 1234:1111
kabi: PCI walk: 'bochs-drm' probe returned -19
```

`make ARCH=arm64 test-module-k20` is the new regression target.
**21 kABI tests** pass.

Two probe firings, one of them userspace-visible.  And — for
the first time in the kABI arc — a probe that **fails**.  In an
informative way.

## The success: /dev/dri/card0

cirrus's probe traversed the same 11-call chain we proved out
in K19, but with one new behavior at the tail: K17's
`drm_dev_register` stub got upgraded to actually do something.

```rust
const DRM_MAJOR: u32 = 226;
static NEXT_DRM_MINOR: AtomicU32 = AtomicU32::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn drm_dev_register(_dev: *mut c_void, _flags: u64) -> i32 {
    let minor = NEXT_DRM_MINOR.fetch_add(1, Ordering::Relaxed);
    let card_name = format!("card{}", minor);
    install_chrdev_in_subdir(DRM_MAJOR, minor, 1, "dri",
                             &card_name, &DRM_FOPS_ADAPTER);
    0
}
```

Underneath: a new `install_chrdev_in_subdir()` helper threads
through `add_runtime_file_in_subdir()` in devfs, which lazily
creates the `/dev/dri` directory via a new `get_or_add_dir()` in
tmpfs (idempotent — `add_dir` overwrites, which would have
broken multi-card registration).

The `DRM_FOPS_ADAPTER` is a static `FileOperationsShim` whose
slots wrap the K17 drm_open/_release/_read/_poll stubs into
Kevlar's K4 char-device callback signatures.  When userspace
later opens `/dev/dri/card0`, the trip is:

```
sys_open("/dev/dri/card0")
  → tmpfs lookup
  → KabiCharDevFile::open()  [K4 adapter]
  → DRM_FOPS_ADAPTER.open    [K20 wrapper]
  → drm_open(inode, filp)    [K17 stub: returns 0]
```

K21+ replaces those K17 stubs with real DRM ioctl dispatch.
But today, opening `/dev/dri/card0` succeeds — even if it does
nothing useful yet.

That's the userspace-visible promise of the kABI arc beginning
to land: **a real Linux DRM driver registered a real device
node, visible to a regular `open()` syscall, in a non-Linux
kernel.**

## The failure: bochs returned -19

bochs is the second real DRM driver Kevlar loads.  It registers
identically to cirrus, and K20 added a fake bochs PCI device
(vendor=0x1234, device=0x1111) so its probe could fire too.
And it does fire — but returns -ENODEV.

The disassembly of `bochs_pci_probe` shows why:

```asm
bl <aperture_remove_conflicting_pci_devices>  ; K17 stub → 0  ✓
cbz w0, +0x84                                  ; not zero → continue
bl <__devm_drm_dev_alloc>                      ; K19 alloc → real buf  ✓
bl <pcim_enable_device>                        ; K17 stub → 0  ✓
bl <bochs_hw_init>                             ; ★ internal — fails
cbz w0, +0x12c                                 ; not zero → error path
bl <drm_dev_put>                               ; cleanup
ret -ENODEV
```

`bochs_hw_init` reads the fake BAR2 mmio (which we provided as
zeroed memory), then issues a `bochs_dispi_read(BOCHS_DISPI_INDEX_ID)`
which expects the bochs adapter's magic ID `0xB0C0`.  Gets back
`0x0000`.  Mismatch → -ENODEV → cleanup → return.

This is the **first probe in the kABI arc that explicitly fails
at a hardware check**.  It's a useful failure: it tells us
exactly what K21 would need to do to make bochs succeed —
mock the bochs MMIO so register reads at offset 0xa return
0xB0C0, or accept "bochs probe didn't find hardware" as the
right answer for an emulated environment without a real Bochs
adapter.

## What changed under the hood

Beyond the user-visible new char device, K20 added three small
infrastructure pieces:

**1.  `kevlar_tmpfs::Dir::get_or_add_dir()`**: a 6-line addition
that turns `add_dir`'s "always create new" semantics into
"create-or-reuse."  Without this, registering a second card
would have wiped out the first one's `/dev/dri` and orphaned its
file entry.  Linux solves this with a dentry cache; ours is
just a lookup-then-insert.

**2.  `DevFs::add_runtime_file_in_subdir()`**: 6 more lines.
Composes `get_or_add_dir` with `add_file`.  Used by
`install_chrdev_in_subdir`.

**3.  DRM fops adapter**: 30 lines of `extern "C" fn`
boilerplate that bridges Kevlar's K4 `FileOperationsShim`
function-pointer signatures (`*mut FileShim, *mut u8, ...`) to
the K17 DRM stub signatures (`*mut c_void, *mut c_void, ...`).
The two are identical at the bit level but Rust's type system
needs explicit casts at the boundary.  When real DRM ioctls
land in K21, this is where they hang.

## Why the bochs failure is interesting

K1-K19 had a clean linear story: every milestone increased the
"loaded" count.  K20 introduced the first **deliberate
failure mode** — a probe that fires, runs partway through, and
returns an error code that propagates back up to our PCI
walker.

This is structurally important.  Real Linux kernels handle
probe failures all the time (driver tries to bind, hardware
isn't actually present, driver gracefully bails).  The fact
that Kevlar correctly logs `'bochs-drm' probe returned -19`
means the **error-path machinery** works:

- `__devm_drm_dev_alloc` allocated a buffer.  bochs's failure
  path called `drm_dev_put` (K18 stub: no-op).  The buffer
  leaked — fine, K30+ when device removal is real.
- The probe's return value (`-ENODEV`) propagated up through
  our `walk_and_probe` orchestrator, which logged it
  faithfully.
- Other probes weren't affected.  cirrus had already succeeded;
  the bochs failure didn't disturb it.

The error path is exercised.  When K21+ has multiple drivers,
some succeed, some fail, the orchestrator keeps walking.

## What didn't have to be done

- **DRM ioctl dispatch.**  `/dev/dri/card0` opens, but every
  ioctl returns 0 (or via the K17 stub, nothing meaningful).
  Userspace `drmModeGetResources()` would not work yet.  K21+.
- **Real DRM minor management.**  K20 just counter-allocates
  starting from 0; Linux uses a complex IDR for hot-plugging.
- **Real driver-private fops.**  cirrus's actual fops (with
  driver-specific ioctl handlers) aren't dispatched.  K21+ when
  ioctls actually fire, we'll need to look up the driver-side
  fops, not the kABI shared adapter.
- **Sysfs nodes for /sys/class/drm/cardN.**  We don't have
  meaningful sysfs.  Userspace tools that walk `/sys/class/drm`
  would not find these devices.  K30+ if needed.

## Cumulative kABI surface (K1-K20)

~331 exports.  ~50 shim modules.  Six DRM modules loadable.
**Two driver probes have run; one userspace-visible
`/dev/dri/cardN` exists.**

## Status

| Surface | Status |
|---|---|
| K1-K19 | ✅ |
| K20 — /dev/dri/card0 + bochs probe fires | ✅ (cirrus full, bochs partial) |
| K21+ — DRM ioctl dispatch + bochs hw-mock | ⏳ |

## What K21 looks like

Two natural threads, in priority order for "graphical ASAP":

1. **Real DRM ioctl dispatch**: when userspace opens
   `/dev/dri/card0` and calls `DRM_IOCTL_VERSION` or
   `DRM_IOCTL_GET_CAP`, the call needs to go to the *driver's*
   fops table (cirrus's specific handler), not Kevlar's shared
   no-op stub.  This requires per-card driver-fops lookup at
   open() time.  Bridge for fbcon / Xorg / DRM userspace tools.
2. **Mock bochs hardware ID** so bochs probe also succeeds and
   `/dev/dri/card1` appears.  Smaller — the read of bochs
   register `BOCHS_DISPI_INDEX_ID` is a `dispi_read(reg=0)` then
   `dispi_read(reg=10)` pair; the second one expects 0xB0C0.
   Add a `mmio_read` hook to our fake BAR2 that returns the
   right byte at the right offset.  Validates the multi-card
   path end-to-end.

Either way, K21 is moving toward the userspace half of
graphical Alpine.  Xorg's DRM open-and-ioctl dance is the next
real exercise — and that's the test where K17's stubs really
will need to upgrade.

The "graphical ASAP" arc is **4 milestones away**.  K20 was
where userspace first saw a kABI-driven device.
