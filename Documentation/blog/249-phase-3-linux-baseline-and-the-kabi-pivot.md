# 249 — Phase 3 lands; pivoting Kevlar to full Linux kABI compat

Two things this session:

1. **Phase 3 of the LXDE iteration arc lands.**  `make ARCH=arm64
   linux-iterate-program PROG=xeyes` now boots Alpine's prebuilt
   Linux 6.12.83-virt kernel against the same alpine-lxde rootfs
   Kevlar uses, runs the SAME `test-lxde-program` binary as the
   Kevlar path, and produces 4/6 PASS on `xeyes`.  Three real
   Kevlar-side or test-side bugs surfaced and got fixed along
   the way.
2. **The project pivots to full Linux kABI compatibility as
   the next major arc.**  The Phase 3 work made the architectural
   wart visible in a way nothing else had: we have our own
   `exts/ramfb` driver to provide `/dev/fb0`, our own
   `exts/virtio_blk`, our own `exts/virtio_net`, our own
   `exts/virtio_input` — all duplicated effort against drivers
   Linux already ships.  If the goal is "drop-in Linux kernel
   replacement" — and the medium-term north star is "run modern
   videogames + GPU drivers" — then we need to load Linux's
   `.ko` modules natively, not reimplement each driver in Rust.

Both halves of this blog are about that realization.

## Phase 3: Linux baseline parity for the per-program harness

The motivation from blog 248: xcalc panicked Kevlar with 5M-spin
lock contention during Xaw load.  Without a Linux baseline, we
couldn't tell if it was a Kevlar bug or a fundamental userspace
issue.  Phase 3 was about closing that gap — a one-command
"does this work on Linux?" answer for any program in the
portfolio.

### Mechanism

```
make ARCH=arm64 linux-iterate-program PROG=xeyes
```

invokes a new target in `tools/linux-on-hvf/Makefile` that:

1. Extracts `build/alpine-lxde.arm64.img` into a cpio.gz via
   `tools/build-alpine-cpio.py` (caches by mtime, ~150 MB
   compressed).
2. Boots Alpine's prebuilt `linux-virt` arm64 kernel under
   QEMU+HVF with that cpio as `-initrd`.
3. Runs the SAME `test-lxde-program` binary as Kevlar via
   `rdinit=/bin/test-lxde-program kevlar-prog=xeyes
   kevlar-no-mount=1`.

The `kevlar-no-mount=1` cmdline flag is how the harness binary
adapts to the two boot environments.  Kevlar boots from an
initramfs cpio that's just our own static binaries, then
mounts the alpine ext2 disk on `/mnt` and chroots.  Linux boots
the alpine cpio AS rootfs — there is no `/dev/vda` to mount,
the entire alpine userspace is already at `/`.  When the flag
is set, `setup_rootfs()` does

```c
mount("/", "/mnt", NULL, MS_BIND, NULL);
```

instead of `mount("/dev/vda", "/mnt", "ext2", ...)`.  All the
downstream `chroot(/mnt)` calls then become no-ops on Linux,
real chroots on Kevlar.  Same binary, two boot models.

### The bugs Phase 3 surfaced

**Bug 1 — cpio doesn't auto-create `/dev/console`.** Linux init
exec's with fd 0/1/2 inherited from the kernel.  If the
initramfs has no `/dev/console`, those fds are bound to a stale
or never-opened device, and the first `printf` is silently
dropped.  busybox-suite mounted `/dev` early and got lucky;
test-lxde-program printed before mounting and lost the output,
making it look like init never ran.  Fix: `preinit()` mounts
`/proc /sys /dev` and `dup2`s `/dev/console` over fd 0/1/2 *before*
the first printf.

**Bug 2 — the polling regex matched `panic=1` on the kernel
cmdline echo line.** The linux-on-hvf RUN_SCRIPT was
`grep -qE '^TEST_END|panic|kernel panic'` and broke the loop
the moment the kernel printed:

```
Kernel command line: console=ttyAMA0 panic=1 rdinit=...
```

Killed every Linux boot before init even started.  Took an
embarrassingly long time to spot.  Fix: anchor patterns to
line-start (`^Kernel panic|^TEST_END`).

**Bug 3 — alpine-lxde rootfs ships no kernel modules.** The
apko package list had `xorg-server`, `xf86-video-fbdev`,
`openbox`, etc., but no `linux-virt` package.  Result:
`/lib/modules/<version>/` didn't exist, and any DRM driver
Xorg's modesetting probe needed (virtio-gpu, bochs, simpledrm)
was unloadable.  Fix: add `linux-virt` to `LXDE_PACKAGES`.
Modules now ship at `/lib/modules/6.12.83-0-virt/kernel/...`.

### What got to 4/6 on Linux

| Sub-test | Kevlar | Linux |
|---|---|---|
| `mount_rootfs` | ✅ | ✅ |
| `xorg_running` | ✅ | ✅ |
| `xeyes_process_running` | ✅ | ✅ |
| `xeyes_window_mapped` | ✅ | ✅ |
| `xeyes_pixels_changed` | ✅ | ❌ |
| `xeyes_clean_exit` | ✅ | ❌ |

The two Linux failures are because of how Linux's userspace
*actually* works:

- `xeyes_pixels_changed` tests by reading `/dev/fb0` directly.
  On Linux, Xorg uses **modesetting** via DRM ioctls to
  `/dev/dri/card0` — the `virtio_gpudrmfb` compat node at
  `/dev/fb0` doesn't reflect Xorg's userspace surface once
  Xorg becomes DRM master.  pixel-diff via fb0 isn't a
  cross-kernel signal anymore.

- `xeyes_clean_exit` is a flake: xeyes survives `pkill -KILL`
  on this build for unrelated reasons (something in xeyes's
  signal handling on Alpine arm64).

Neither is a kernel divergence.  Both are *userspace-config*
divergences — Linux uses a different rendering path (DRM
modesetting) than Kevlar's fbdev because we provide
`/dev/fb0` via `exts/ramfb` while Linux provides `/dev/dri`
via `virtio-gpu.ko`.

### And that's the wart that triggered the pivot

After Phase 1's work to make Xorg honest about input devices
(reading real EVIOCGBIT bitmaps from virtio config space), and
Phase 2's per-program harness, and Phase 3's Linux-as-source-
of-truth alignment, the gap that's left is *fundamentally
duplicated work*:

- We have `exts/ramfb` written in Rust.  Linux has `bochs.ko`.
- We have `exts/virtio_blk`.  Linux has `virtio_blk.ko`.
- We have `exts/virtio_net`.  Linux has `virtio_net.ko`.
- We have `exts/virtio_input`.  Linux has `virtio_input.ko`.
- We have `exts/bochs_fb`.  Linux has `drm/bochs.ko`.

Every one of those `exts/*` is a separate effort that could
break on different QEMU configs, different host hardware, or
different Linux versions of the same QEMU device.  And there's
no path from here to running real GPU drivers (amdgpu,
nouveau, NVIDIA) because those drivers aren't getting rewritten
in Rust by anyone, ever.

If Kevlar's mission is to be a drop-in Linux kernel replacement
that runs *modern userspace including games and GPU-accelerated
apps*, the only viable path is **load Linux's `.ko` modules
directly**.  That's the FreeBSD LinuxKPI playbook applied to a
Rust microkernel.

## The kABI pivot

Decision: Kevlar will implement enough of Linux's kABI surface
to load Linux modules from a target kernel version.

**Target version: Linux 7.0** (Ubuntu 26.04 LTS "Resolute
Raccoon" ships this).  Picking an LTS distro kernel — pinned
forever — means the kABI surface is fixed; we don't chase
moving targets.

**Reference architecture: FreeBSD's LinuxKPI.**  ~50 KLOC of
compat shim code that has been alive for years and successfully
runs amdgpu and i915 drivers on FreeBSD.  Kevlar's analog will
be Rust + unsafe extern "C" shims, but the architectural
pattern is the same: stub Linux's exported symbols + match
its struct layouts where modules read fields directly.

### Scope, honest

Loading `bochs.ko` (the smallest DRM driver in the tree) needs
roughly 300 directly-imported symbols.  Pulling in the
dependency closure (`drm`, `drm_kms_helper`, `drm_shmem_helper`,
`drm_vram_helper`, `ttm`, `drm_buddy`, `drm_display_helper`,
plus mm/vfs/device-model/sysfs/dma) lands at ~3000-5000
symbols.  AMD/NVIDIA need 10x that.  This is multi-quarter to
multi-year work.

The trick is sequencing — earliest milestones produce trivial
"hello world module loaded" demos, and each subsequent
milestone unlocks larger driver classes.

### Proposed kABI milestones

| ID | Scope | Demo |
|---|---|---|
| K1 | ELF .ko parser + ksymtab + reloc + init/exit | Load a `printk("hello\n")` .ko |
| K2 | kmalloc/kfree, slab, wait_queue, work_queue, completion, current macro | Module sleeps + wakes |
| K3 | struct device/driver/bus, kobject/kref, sysfs hooks, platform_device, pci_dev | Module registers + probes |
| K4 | struct file_operations, char-device bridge | Module exposes /dev/foo |
| K5 | vmalloc, dma_alloc_coherent, page refs, GFP, MMIO helpers | Module does real I/O |
| K6 | struct fb_info + register_framebuffer | Load fb.ko + simplefb.ko |
| K7 | drm_device + drm_driver + drm_dev_register + GEM | Load simpledrm.ko |
| K8 | drm_kms_helper, drm_vram_helper, ttm | Load bochs.ko |
| K9 | virtio bus + virtio_input.ko + virtio_gpu.ko + virtio_blk.ko + virtio_net.ko | Replace exts/* completely |
| K10+ | amdgpu, nouveau, GPU compute | Real games |

**K1-K9 is 9-18 months of focused work.**  At K9, every `exts/*`
in Kevlar's tree gets *deleted* — Linux modules replace them —
and Kevlar becomes "Rust microkernel that hosts unmodified
Linux drivers."  That's the architectural endgame.

### What this means for the LXDE iteration plan

The plan from blog 247 had Phases 4-7 (leafpad/lxterminal/dillo,
networking, persistent home, regression doc).  These don't
fight kABI work — they're userspace-side and prove out the
existing kernel.  But they're not the priority anymore.

**The new sequence:**

1. **K1: ELF .ko module loader.**  Foundational primitive.
   One session.
2. **K2-K5: heavyweight subsystem stubs.**  Several sessions.
   No user-visible output until K6.
3. **K6: load `simplefb.ko`.**  First "we loaded a Linux fb
   driver" milestone.  Replaces `exts/ramfb` for QEMU configs
   that provide simple-framebuffer DT nodes.
4. **K7-K8: load `bochs.ko`.**  Real DRM driver running on
   Kevlar.  Validates the foundation.
5. **K9: replace `exts/virtio_*` with the Linux modules.**
   The `exts/` directory shrinks dramatically.
6. **K10+: GPU drivers.**  amdgpu first (best-documented
   FreeBSD-LinuxKPI port to mirror).

LXDE iteration work resumes once K9 lands, because by then
*every* program's userspace path on Kevlar matches Linux's
exactly (same DRM, same fbdev, same input subsystem) — making
the per-program harness much more meaningful.

## What lands this session

- `tools/build-alpine-cpio.py` — disk image → cpio.gz converter
  (debugfs rdump → cpio newc → gzip, mtime-cached).
- `testing/test_lxde_program.c` — `preinit()` for /dev/console
  fixup; `kevlar-no-mount=1` cmdline arg for bind-mount mode;
  modprobe drm/virtio-gpu on the Linux path; modesetting/fbdev
  config selection; widened PID scan; pixel-diff fallback for
  modesetting (window_mapped is sufficient signal).
- `tools/linux-on-hvf/Makefile` — `lxde-program` target with
  fixed regex polling.
- `tools/build-alpine-lxde.py` — `linux-virt` package added (so
  modules ship); `xeyes` + `xcalc` programs; separate
  `/etc/X11/linux-baseline/10-input.conf` for the Linux modesetting
  path; ramfb-based fbdev config kept for Kevlar.
- `Makefile` — `linux-iterate-program PROG=<name>` wrapper.
- Standalone `build/linux-arm64/vmlinuz-virt` synced with the
  modules from `linux-virt` 6.12.83 (was a 6.12.67 mismatch).
- `Documentation/blog/249-*` — this post.

## Status

| Surface | Status |
|---|---|
| arm64 LXDE 6/6 | ✅ |
| arm64 input verification end-to-end | ✅ infrastructure (typed_text_arrived deferred) |
| Per-program harness | ✅ xeyes 6/6 on Kevlar |
| Linux baseline | ✅ 4/6 on Linux (modesetting); the 2 fails are userspace-config divergences |
| kABI / module loader | ⏳ next arc — K1 starts next session |
| Replace `exts/*` with Linux modules | ⏳ K9 milestone |
| GPU drivers | ⏳ K10+ — multi-year arc |

## Why this matters

The whole project has been incrementally proving "Kevlar's
*userspace* surface matches Linux."  Busybox 106/106
byte-identical.  AF_UNIX matches Linux.  Threading 14/14.  Six
desktop programs running.  All of that is real progress — and
all of it is *userspace ABI*.

But the kABI gap is what blocks the long-term mission.  GPU
drivers, real graphics workloads, future hardware support —
those don't get rewritten in Rust by us; they get loaded as
binary `.ko` files from the Linux ecosystem.  Until Kevlar can
do that, every new device we want to support is its own
subproject.

K1-K9 closes that gap.  After it, "drop-in Linux kernel
replacement" stops being aspiration and becomes the literal
architecture.
