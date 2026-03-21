# M10.5: Linux Kernel Module Compatibility (kcompat)

**Goal:** Load unmodified Linux 6.18 LTS kernel modules (`.ko` files) on
Kevlar — enabling real hardware support (NVMe, SATA, USB, WiFi, GPU) without
rewriting drivers in Rust.

**Target kernel:** Linux 6.18 LTS (EOL 2029). The API is frozen within this
series. We update kcompat shim once per LTS release, not per patch.

**Placement:** Between M10 (Alpine text, QEMU) and M11 (Alpine graphical).
kcompat enables real hardware so M11 can run on physical machines, not just
QEMU. virtio-gpu is a fallback for M11; native GPU via kcompat is the goal.

---

## Why kcompat is the right approach

Rewriting every driver in Rust is correct long-term, but the short-term
math doesn't work: `nvme.ko` is ~15K lines; `amdgpu.ko` is ~500K lines.
The Linux driver ecosystem is essentially a moat that took 30 years to build.

kcompat is a **thin translation layer** that maps Linux's internal kernel
API to Kevlar's native equivalents. The kernel module binary links against
kcompat symbols; it never knows it isn't on Linux.

This is not novel: Android's GKI (Generic Kernel Image) does exactly this
to maintain binary module compatibility across vendor kernels. Rust-for-Linux
drivers use a similar abstraction over the Linux C API.

---

## The ABI split

| Layer | Who uses it | Kevlar status |
|-------|-------------|---------------|
| **Syscall ABI** (`read`, `mmap`, `clone`) | Userspace processes | Done — 130+ syscalls |
| **Kernel module ABI** (`request_irq`, `pci_enable_device`, DRM) | `.ko` drivers | This milestone |

Syscall compatibility (M1-M10) does NOT help with driver loading. These are
completely separate interfaces.

---

## Driver tiers

| Tier | Drivers | Complexity | Timeline |
|------|---------|------------|----------|
| **1: Storage + NICs** | nvme, ahci, e1000e, r8169, iwlwifi | ~100-300 symbols each | Months |
| **2: Open GPU** | amdgpu, i915 (display + 3D) | ~800+ symbols, DRM/TTM/HMM | 1-2 years |
| **3: Proprietary GPU** | nvidia.ko | Opaque binary, own version checks | Very hard (stretch) |

Start with Tier 1 (storage + NICs) to enable real hardware boot. Add GPU
incrementally. NVIDIA is stretch — the userspace-only path (UVM ioctls +
Mesa NVK) is more tractable.

---

## Architecture

```
Linux .ko module (ELF ET_REL, compiled for 6.18)
         │
         │  calls Linux kABI symbols (request_irq, pci_read_config_dword, ...)
         ▼
kevlar-kcompat   (new crate: kernel/kcompat/)
         │  - exports matching symbol CRCs for Linux 6.18
         │  - implements struct layouts from Linux 6.18 headers
         │  - translates Linux API calls to Kevlar-native equivalents
         ▼
Kevlar kernel internals
  (IRQ registration, PCI config space, DMA mapping, block layer, ...)
```

The kcompat crate is pure Rust with `#[allow(unsafe_code)]` because it must
handle foreign .ko binaries calling into it.

---

## Phases

| Phase | Deliverable | Milestone |
|-------|-------------|-----------|
| 1: Module loader | ELF .ko loading, symbol resolution, vermagic | Load hello-world.ko |
| 2: Core subsystems | PCI, IRQ, DMA, work queues, device model, kmalloc | lspci-equivalent works |
| 3: Storage drivers | nvme.ko + ahci.ko, block layer shim | Boot from NVMe/SATA |
| 4: USB HCD | xhci-hcd.ko, usb-storage.ko | USB flash drive mounts |
| 5: Network drivers | e1000e.ko, r8169.ko, iwlwifi.ko | Real NIC up, DHCP, ping |
| 6: DRM/KMS | DRM core + KMS (amdgpu/i915 display only) | Display output on monitor |
| 7: GPU acceleration | TTM, GEM, mmu_notifier, Mesa/RADV | glmark2, Vulkan |

**Estimated total:** 2-3 years for Phases 1-6; Phase 7 is ongoing.

---

## Symbol versioning (CRC matching)

Linux modules embed CRC checksums for every imported symbol. `insmod`
rejects modules where the CRC doesn't match the running kernel's export.

For 6.18, we pre-compute the exact CRCs (from `Module.symvers`) and embed
them in kcompat's export table. This is deterministic — `Module.symvers`
is an artifact of the kernel build and is fixed for a given version.

When Linux 6.19 LTS is released, we update the CRC table and any struct
fields that changed. For LTS-to-LTS updates, the delta is typically
1-20 struct field changes in the relevant subsystems.

---

## Struct layout compatibility

Linux modules access kernel structs by compiled-in field offsets. If
`struct pci_dev` has `irq` at offset 0x58 in 6.18, kcompat's `struct pci_dev`
must have `irq` at exactly 0x58.

Strategy:
1. Copy the relevant struct definitions verbatim from Linux 6.18 headers
2. Add `#[repr(C)]` and `static_assert!(offset_of!(pci_dev, irq) == 0x58)`
3. Map struct fields to Kevlar-native types where needed (e.g., `spinlock_t`
   → a no-op wrapper or Kevlar SpinLock depending on how the driver uses it)

---

## What kcompat does NOT need

- **Sysfs**: mdev/udev use sysfs; kcompat can stub sysfs writes for modules
  that register attributes (they don't fail if sysfs is a no-op)
- **IOMMU PASID**: Required for HMM/unified GPU memory (Phase 7), not Tier 1
- **mmu_notifier**: GPU coherent memory (Phase 7), not storage/NICs
- **debugfs**: Stub — `debugfs_create_*` returns NULL, drivers handle this

---

## Success criteria

- [ ] Phase 1: `insmod hello.ko` prints "Hello, Kevlar!" and `rmmod hello` unloads
- [ ] Phase 3: Alpine boots from NVMe SSD or SATA HDD on real x86_64 hardware
- [ ] Phase 4: USB flash drive mounts read-write
- [ ] Phase 5: Real NIC gets DHCP IP, `apk update` downloads packages
- [ ] Phase 6: Monitor displays framebuffer output via real GPU
- [ ] Phase 7: `vulkaninfo` reports GPU, `glmark2` renders frames
