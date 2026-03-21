# M10.5 Phase 7: GPU Acceleration

**Goal:** Full GPU acceleration via `amdgpu.ko` or `i915.ko`. Mesa/RADV
or ANV Vulkan ICD renders frames. `glmark2` runs; `vulkaninfo` shows GPU.

This is the largest and most complex phase. It requires TTM (GPU memory
manager), HMM/mmu_notifier (unified memory), IOMMU PASID, and DMA-BUF
cross-device sharing.

---

## What Phase 6 didn't include

Phase 6 (DRM/KMS) covered:
- Display modesetting (CRTC, connector, scanout)
- Dumb/CMA framebuffers (linear, CPU-writable)
- Basic GEM object lifecycle

Phase 7 adds:
- **TTM** (Translation Table Manager) — GPU VRAM, GTT, eviction, migration
- **Render nodes** (`/dev/dri/renderD128`) — 3D rendering without display access
- **Command submission** — GPU command buffers, synchronization fences
- **mmu_notifier** — CPU page table change notifications for unified memory
- **IOMMU PASID** — per-process GPU IOMMUs (for SVM/HMM)
- **DMA-BUF** — cross-device buffer sharing (GPU → display, GPU → encoder)

---

## TTM (Translation Table Manager)

TTM manages GPU memory across three placement types:

| Placement | What it is | Notes |
|-----------|-----------|-------|
| **VRAM** | On-GPU memory (fast) | Physical GPU VRAM |
| **GTT** | System RAM mapped into GPU's GART/IOMMU | Slower than VRAM |
| **CPU** | Normal system RAM | Not GPU-accessible without mapping |

TTM handles:
- Buffer object allocation in each placement
- **Eviction**: move VRAM → GTT when VRAM is full (paging)
- **Migration**: move GTT → VRAM when buffer is needed on GPU
- **Pinning**: prevent eviction while in use
- **Swapping**: GTT → swap file when system RAM is full

### Key structs and functions

```c
struct ttm_buffer_object {
    struct ttm_device *bdev;
    struct ttm_resource *resource;  // current placement
    struct ttm_place *places;
    // ...
};
```

| Function | Implementation |
|----------|----------------|
| `ttm_device_init(bdev, ...)` | Initialize TTM device |
| `ttm_bo_init_reserved(bdev, bo, size, ...)` | Allocate buffer object |
| `ttm_bo_put(bo)` | Release reference |
| `ttm_bo_pin(bo)` | Pin in current placement |
| `ttm_bo_unpin(bo)` | Unpin |
| `ttm_bo_evict_mm(bdev, mem_type)` | Evict all buffers from placement |
| `ttm_mem_global_alloc(mem_glob, size, ...)` | Account memory usage |

---

## Command submission and fences

GPU commands are submitted as command buffers (IBs — Indirect Buffers):

```
userspace (Mesa) → DRM_IOCTL_AMDGPU_CS (command submission)
        │
        ▼
amdgpu driver: validate IBs, add to ring buffer
        │
        ▼
GPU executes commands
        │
        ▼
GPU signals fence (via interrupt or memory write)
        │
        ▼
DRM_IOCTL_AMDGPU_WAIT_CS / dma_fence_wait()
```

### DMA fences

`struct dma_fence` is Linux's GPU synchronization primitive:

| Function | Implementation |
|----------|----------------|
| `dma_fence_init(fence, ops, lock, context, seqno)` | Initialize fence |
| `dma_fence_signal(fence)` | Signal completion |
| `dma_fence_wait(fence, intr)` | Wait for completion |
| `dma_fence_get/put(fence)` | Reference counting |
| `dma_fence_chain_alloc/init` | Fence chains (timeline semaphores) |

Fences are used by Wayland compositors for GPU/display synchronization
(explicit sync via `EGL_ANDROID_native_fence_sync`).

---

## mmu_notifier

The most complex dependency for GPU unified memory (HMM — Heterogeneous
Memory Management):

When the CPU's page table changes (munmap, fork, mprotect, page reclaim),
the GPU must be notified so it can:
1. Invalidate its own page tables (GPU TLB flush)
2. Remove mappings for pages being unmapped
3. Handle page migration between CPU and GPU VRAM

```c
struct mmu_notifier_ops {
    void (*invalidate_range_start)(struct mmu_notifier *mn,
                                   const struct mmu_notifier_range *range);
    void (*invalidate_range_end)(struct mmu_notifier *mn,
                                 const struct mmu_notifier_range *range);
    // ...
};
```

kcompat implementation: hook into Kevlar's VM teardown (`munmap`, `mprotect`,
`exec`) to call any registered mmu_notifier callbacks before modifying
page tables.

This requires modifying Kevlar's VM core (`kernel/mm/vm.rs`) to call back
into kcompat. It's a two-way dependency but manageable.

---

## IOMMU PASID (stretch)

PASID (Process Address Space ID) allows the GPU to use per-process IOMMUs —
each process gets its own IOMMU page table, so the GPU can access process
virtual addresses directly.

This is required for:
- AMD SVM (Shared Virtual Memory) — GPU access to CPU VAs without memcpy
- Intel SVM (similar)

For M10.5 Phase 7, PASID is optional. Most workloads don't need SVM.
Implement IOMMU PASID in M12 if needed.

---

## DMA-BUF and PRIME

DMA-BUF is a cross-device buffer sharing mechanism:
- GPU renders into a buffer
- Buffer exported as an fd (`DRM_IOCTL_PRIME_HANDLE_TO_FD`)
- Display subsystem imports the fd (`DRM_IOCTL_PRIME_FD_TO_HANDLE`)
- Scanout happens directly from GPU VRAM — no copy

This is essential for Wayland: the compositor receives DMA-BUF fds from
clients and scanouts them directly.

Phase 6 had stubs; Phase 7 implements real DMA-BUF sharing between
`amdgpu`/`i915` and the display pipeline.

---

## Mesa/Vulkan path

Mesa's RADV (AMD Vulkan) and ANV (Intel Vulkan) talk to the GPU driver via
DRM ioctls. Key ioctls:

| ioctl | Purpose |
|-------|---------|
| `DRM_IOCTL_AMDGPU_INFO` | Query GPU caps, VRAM size, PCI info |
| `DRM_IOCTL_AMDGPU_GEM_CREATE` | Allocate GPU buffer (backed by TTM) |
| `DRM_IOCTL_AMDGPU_GEM_MMAP` | Map GPU buffer to userspace |
| `DRM_IOCTL_AMDGPU_CTX` | Create command submission context |
| `DRM_IOCTL_AMDGPU_BO_LIST` | Register buffer list for CS |
| `DRM_IOCTL_AMDGPU_CS` | Submit command buffer |
| `DRM_IOCTL_AMDGPU_WAIT_CS` | Wait for submission completion |
| `DRM_IOCTL_AMDGPU_GEM_VA` | Manage GPU VA mappings |
| `DRM_IOCTL_AMDGPU_FENCE_TO_HANDLE` | Export fence as syncobj |
| `DRM_IOCTL_SYNCOBJ_CREATE/WAIT/SIGNAL` | Timeline semaphores |

All of these come from the `amdgpu.ko` driver (kcompat handles routing
to the driver). Kevlar only needs to dispatch the ioctl to the loaded module.

---

## NVIDIA (stretch goal)

`nvidia.ko` is a special case:

1. **Opaque binary** — no source, no headers for its internal functions
2. **Own version detection** — rejects non-Linux kernels explicitly
3. **UVM** (`nvidia-uvm.ko`) — separate module for unified memory
4. **Wrapper layer** — NVIDIA ships an open-source wrapper (`nvidia-open`)
   that bridges their closed GPU implementation to the Linux kernel API

The `nvidia-open` wrapper (open since 2022) is the right approach:
- Provides the kernel interface in open source
- The closed binary is only the GPU firmware/microcode side
- Adapting the wrapper to Kevlar's kcompat is more feasible than the
  old opaque binary approach

Still very hard. Defer until after AMD/Intel GPU is working.

---

## Verification

### Basic render test

```bash
insmod amdgpu.ko
ls /dev/dri/  # card0 + renderD128
# Vulkan test
vulkaninfo 2>&1 | head -30
# OpenGL test
glmark2 --fullscreen
```

### Wayland compositor

```bash
# Sway (Wayland compositor)
sway
# Should start with GPU-accelerated rendering
# Firefox, terminals, etc. run with hardware acceleration
```

---

## Dependencies summary

| Dependency | Phase | Needed for |
|-----------|-------|-----------|
| DRM core + KMS | Phase 6 | All GPU |
| TTM memory manager | Phase 7 | GPU buffer allocation |
| DMA-BUF/PRIME | Phase 7 | Compositor buffer sharing |
| mmu_notifier | Phase 7 | Unified memory (optional initially) |
| IOMMU PASID | M12 | SVM (optional) |

---

## Files to create/modify

- `kernel/kcompat/ttm.rs` — TTM buffer objects, eviction, migration
- `kernel/kcompat/dma_fence.rs` — DMA fence synchronization
- `kernel/kcompat/dma_buf.rs` — DMA-BUF cross-device sharing
- `kernel/kcompat/mmu_notifier.rs` — CPU page table change notifications
- `kernel/mm/vm.rs` — hook mmu_notifier callbacks into VM operations
- `kernel/kcompat/syncobj.rs` — DRM sync objects (timeline semaphores)
- `kernel/kcompat/symbols_6_18.rs` — add TTM/fence/DMA-BUF symbols (~200)
