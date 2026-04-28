# 265 — kABI K16: drm_dma_helper.ko, the 60% compounding ratio

K16 lands.  Ubuntu 26.04's `drm_dma_helper.ko` — the helper
layer for DMA-coherent GEM buffers — loads in Kevlar with all
79 symbols resolved.

```
kabi: loading /lib/modules/drm_dma_helper.ko (Ubuntu 26.04)
kabi: loaded /lib/modules/drm_dma_helper.ko (47529 bytes, 41 sections, 289 symbols)
kabi: /lib/modules/drm_dma_helper.ko license=Some("GPL")
       desc=Some("DRM DMA memory-management helpers")
kabi: applied 387 relocations (43 trampoline(s))
kabi: drm_dma_helper is a library module (no init_module)
```

`make ARCH=arm64 test-module-k16` is the new regression target.
17 kABI tests now pass.  **Four DRM helpers** now coexist
(drm_buddy + drm_exec + drm_ttm_helper + drm_dma_helper).

## The compounding payoff at full strength

drm_dma_helper.ko has the largest undef list yet — **79 symbols**.
That should be a big milestone.  But by the time K16 attempted
the load, **47 of those 79 were already stubbed by K1-K15**.

```
total undefs:      79
already stubbed:   47   (drm_fb_helper, drm_client, fb, sys_raster,
                          mutex, module_ref, drm dbg, ubsan,
                          stack_chk_fail, kmalloc/kfree, memcpy,
                          drm_printf, refcount_warn_saturate, …)
net new:           32
```

That's a **60% compounding ratio** — the highest of the kABI
arc.  The last six milestones (K11 dummy, K12 virtio_input, K13
drm_buddy, K14 drm_exec, K15 drm_ttm_helper, K16 drm_dma_helper)
have built up enough kABI surface that subsequent modules find
most of what they need already there.

This is the LinuxKPI shape arriving:
- K11 (network dummy): every undef was new.
- K15 (drm_ttm_helper): 7 of 47 were already there (15%).
- K16 (drm_dma_helper): 47 of 79 already there (60%).

By K17 (real DRM driver) the ratio should keep climbing.

## drm_dma_helper.ko's 32 net-new symbols, classified

```
DMA API  (10):                     DRM GEM  (9):
  dma_alloc_attrs                    drm_gem_create_mmap_offset
  dma_alloc_pages                    drm_gem_fb_get_obj
  dma_buf_vmap_unlocked              drm_gem_handle_create
  dma_buf_vunmap_unlocked            drm_gem_object_init
  dma_free_attrs                     drm_gem_object_release
  dma_free_pages                     drm_gem_prime_mmap
  dma_get_sgtable_attrs              drm_gem_private_object_init
  dma_mmap_attrs                     drm_gem_vm_close
  dma_mmap_pages                     drm_gem_vm_open
  __dma_sync_single_for_device

DRM prime  (3):                    mm helpers  (4):
  drm_mode_size_dumb                 __vma_start_write
  drm_prime_gem_destroy              is_vmalloc_addr  ← real impl
  drm_prime_get_contiguous_size      memstart_addr (static)
                                     vm_get_page_prot
DRM atomic  (2):
  drm_atomic_helper_damage_iter_init      drm_format extension  (2):
  drm_atomic_helper_damage_iter_next        drm_format_info_block_height
                                            drm_format_info_block_width
drm_client extension  (2):
  drm_client_buffer_vmap
  drm_client_buffer_vunmap
```

3 new shim files (`drm_prime.rs`, `drm_atomic.rs`, `mm.rs`),
4 file extensions (`dma.rs`, `drm_gem.rs`, `drm_client.rs`,
`drm_format.rs`).

## The one real implementation in K16

Most of K16 is no-op stubs (drm_dma_helper has no `init_module`,
so nothing fires at load).  But one helper is **real**:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn is_vmalloc_addr(addr: *const c_void) -> bool {
    super::alloc::is_vmalloc_addr_internal(addr as usize)
}
```

`is_vmalloc_addr_internal` is a thin wrapper over K2's
`VMALLOC_TABLE` — the side-table that already tracks vmalloc'd
allocations to make `kvfree` work.  It walks the table and
returns whether the address falls inside any recorded vmalloc
region:

```rust
pub fn is_vmalloc_addr_internal(addr: usize) -> bool {
    VMALLOC_TABLE.lock().iter().any(|e| {
        let lo = e.va;
        let hi = e.va + e.num_pages * PAGE_SIZE;
        addr >= lo && addr < hi
    })
}
```

Why bother with a real implementation when nothing calls it at
K16 load?  Because **future modules will**.  `is_vmalloc_addr`
is a standard Linux predicate — once K17+ probes start firing,
any code path doing `kvfree`-style branching will rely on this.
Build it correctly once.

## The aarch64 `memstart_addr` decision

`memstart_addr` is the only K16 symbol exported as a *static*,
not a function.  It's aarch64's direct-map base — a `u64`
exported by Linux's mm code.  Real value depends on KASLR
slide + the `CONFIG_ARM64_VA_BITS` choice.

K16 exposes 0:

```rust
#[unsafe(no_mangle)]
pub static memstart_addr: u64 = 0;
crate::ksym_static!(memstart_addr);
```

This is a deliberate cheat.  Linux callers that read
`memstart_addr` typically use it for pointer arithmetic
(`virt - memstart_addr` to get a phys offset).  K16 has no
caller that does this.  When K17+ surfaces a probe path that
dereferences `memstart_addr`, we expose the real value.  Until
then, "0" is the smallest valid stub.

## What's left in DRM-helper-land

drm_dma_helper appears to be the **last big DRM helper** Ubuntu
ships as a `.ko`.  Looking at `drivers/gpu/drm/*.ko` in the
package:

- drm_buddy.ko ✅ (K13)
- drm_exec.ko ✅ (K14)
- drm_ttm_helper.ko ✅ (K15)
- drm_dma_helper.ko ✅ (K16)
- drm.ko (built-in `=y`)
- drm_kms_helper.ko (built-in `=y`)
- drm_display_helper.ko (built-in `=y`)
- ttm.ko (built-in `=y`)

The other helpers we'd want are **built into the kernel
binary**, not loadable.  K17+ has to either (a) attempt a real
DRM driver and stub out drm.ko's exports as kABI surface, or
(b) find a small DRM driver that doesn't depend on drm.ko (none
exist; every DRM driver needs drm core).

## Cumulative kABI surface (K1-K16)

~232 exported symbols (K15's 200 + 32 K16 net new).  47+ shim
modules in `kernel/kabi/`.  Four DRM helpers loadable.

## Status

| Surface | Status |
|---|---|
| K1-K15 | ✅ |
| K16 — drm_dma_helper.ko + 32 DMA/GEM/mm stubs | ✅ |
| K17+ — real DRM driver + drm.ko-equivalent stubs | ⏳ |

## What K17 looks like

K17 is the inflection point.  Up to K16 every milestone has
been "load a `.ko`, satisfy the linker, no probe runs."  K17
breaks that pattern — it picks a real DRM driver whose
`init_module` actually does something visible (registers a
DRM device, creates `/dev/dri/card0`, etc.).

Candidates from the package:
- **cirrus-qemu.ko** (88 undefs, KMS driver for QEMU's
  emulated Cirrus VGA).  Most natural fit since we're already
  in QEMU.  Depends on drm.ko (built-in in Ubuntu) — we have
  to provide drm.ko-equivalent stubs (~50 more symbols).
- **gm12u320.ko** (USB-attached projector driver — needs USB
  stack).
- **udl.ko** (USB DisplayLink — needs USB stack).
- **vmwgfx.ko** (VMware SVGA — depends on KVM but uses TTM).
- **virtio_gpu.ko** (virtio GPU driver).  Smallest path to a
  *real* driver firing probe — virtio infrastructure already
  partly exists (K12 virtio_input).

K17 most likely picks **virtio_gpu.ko** or **cirrus-qemu.ko**
and works through whatever new stubs they need.  Either way,
the milestone framing changes: instead of "did the linker
satisfy?", it becomes "**did the driver's probe fire?**".  That
needs:

1. **drm.ko-equivalent stubs**: drm_dev_alloc, drm_dev_register,
   drm_mode_config_*, etc.  ~30-50 symbols.
2. **Real device-class registration**: `/dev/dri/card0` visible
   to userspace requires registering a class + char device.  K4
   and K12 work touched this; K17 makes it real.
3. **Layout exactness for `struct drm_device`,
   `struct drm_gem_object`**: probe paths read fields off these
   structs, so the layouts have to match Linux's exactly.

K17 is at least double K16's complexity.  The "one big stub
batch" rhythm of K12-K16 ends here.

After K17:
- **K18-K19**: layout exactness fixes as probe paths surface
  field-offset bugs.
- **K20-K22**: virtio bus walking + virtio_input probe + Xorg
  reading keystrokes.
- **K23+**: graphical Alpine running.

The "graphical ASAP" arc is now **5 milestones away**.  K16 was
the last "stubs only" milestone before the real work begins.
