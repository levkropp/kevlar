# 264 — kABI K15: drm_ttm_helper.ko, 40 fbdev/TTM stubs in one shot

K15 lands.  Ubuntu 26.04's `drm_ttm_helper.ko` — the framebuffer-
emulation helper DRM drivers use to expose `/dev/fb0` on top of
GPU buffers — loads in Kevlar with all 47 symbols resolved.

```
kabi: loading /lib/modules/drm_ttm_helper.ko (Ubuntu 26.04)
kabi: loaded /lib/modules/drm_ttm_helper.ko (29473 bytes, 38 sections, 157 symbols)
kabi: /lib/modules/drm_ttm_helper.ko license=Some("GPL")
       desc=Some("DRM gem ttm helpers") depends=Some("ttm")
kabi: applied 175 relocations (23 trampoline(s))
kabi: drm_ttm_helper is a library module (no init_module)
```

`make ARCH=arm64 test-module-k15` is the new regression target.
16 kABI tests now pass.

## The biggest milestone by file count, the smoothest landing

K15 was the largest single milestone in the kABI arc so far —
**40 net-new exported symbols across nine new shim files and
four extensions**.  Bigger than K12 (30 stubs, 5 files) and
K13 (13 stubs, 5 files).

It also went in faster than any of them.  No detours, no
HVF asserts, no late-stage cascade of missing symbols.  The
**only** build error was a Rust visibility issue — `platform::mem`
was a private module, so `kernel/kabi/mem.rs` had to reach
`memcpy` via an `extern "C"` declaration instead of a Rust
`use` import.  One-line fix.  Everything else compiled and
loaded clean on the first run.

That's the K10/K11/K14 groundwork compounding.  Every
cross-cutting concern (SCS handling, x18 reservation, kmalloc
renames, variadic printk, sched no-ops) is now absorbed into
the kABI runtime.  K15's job was just **mass-produce 40 small
stubs** and the loader did the rest.

## drm_ttm_helper.ko's surface, classified

```
drm_fb_helper_*  (11):           drm_client_*  (5):
  drm_fb_helper_blank             drm_client_buffer_create_dumb
  drm_fb_helper_check_var         drm_client_buffer_delete
  drm_fb_helper_damage_area       drm_client_buffer_vmap_local
  drm_fb_helper_damage_range      drm_client_buffer_vunmap_local
  drm_fb_helper_deferred_io       drm_client_release
  drm_fb_helper_fill_info
  drm_fb_helper_fini             fb_*  (5):
  drm_fb_helper_ioctl              fb_deferred_io_init
  drm_fb_helper_pan_display        fb_deferred_io_cleanup
  drm_fb_helper_set_par            fb_deferred_io_mmap
  drm_fb_helper_setcmap            fb_sys_read
                                   fb_sys_write
sys_raster  (3):
  sys_copyarea                   ttm_bo_*  (3):
  sys_fillrect                     ttm_bo_mmap_obj
  sys_imageblit                    ttm_bo_vmap
                                   ttm_bo_vunmap
drm_format / drm_dbg  (4):
  drm_format_info_bpp            kernel mutex  (2):
  drm_driver_legacy_fb_format      mutex_lock
  drm_print_bits                   mutex_unlock
  __drm_dev_dbg
                                 module refcount  (2):
misc  (4):                         try_module_get  (always success)
  __warn_printk                    module_put       (no-op)
  dev_driver_string
  drm_gem_object_lookup          memcpy / IO  (2):
  vzalloc_noprof                   memcpy
                                   memcpy_toio
already-stubbed (7):
  __stack_chk_fail (K11), __ubsan_handle_load_invalid_value (K12),
  alt_cb_patch_nops (K11), drm_gem_object_free (K14),
  drm_printf (K13), refcount_warn_saturate (K14), vfree (K2)
```

Net new: **40 symbols**.  Already covered: **7**.  Compounding
ratio creeping up — by K20+ a typical helper module's
satisfy-the-linker shape is mostly already-existing stubs.

## What the new files actually do

Nothing.

That's not flippant — it's the design.  drm_ttm_helper has no
`init_module`.  Every one of these 40 functions is referenced
by the module's exported API (`drm_gem_ttm_vmap`,
`drm_fbdev_ttm_driver_fbdev_probe`, etc.) but is only
*invoked* when **another DRM driver** calls into one of those
exports.  No such driver exists in K15's test corpus.  The
load succeeds, the relocations apply, the module sits in
memory awaiting callers that won't arrive until K20+.

So every stub is the simplest thing that satisfies the linker:
- Function returning `i32`?  Return 0.
- Function returning `*mut T`?  Return null.
- Void function?  Empty body.
- Bool?  Return `true`.

The exception is **`memcpy`** and **`memcpy_toio`**, which are
real implementations re-exported from `platform/mem.rs`.  The
`mem.rs` shim is two lines:

```rust
unsafe extern "C" {
    fn memcpy(dest: *mut u8, src: *const u8, n: usize) -> *mut u8;
}
crate::ksym!(memcpy);
crate::ksym_named!("memcpy_toio", memcpy);
```

That puts `memcpy`'s already-existing address into the kABI
export table, then aliases it as `memcpy_toio` (which on
aarch64 at our level is the same operation — IO accesses go
through plain ldr/str at the kABI layer, no special device
mapping required for K15's stub corpus).

## Pattern files: stubs that look the same but mean different things

The 11 `drm_fb_helper_*` callbacks are all variants of the
same shape:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn drm_fb_helper_blank(_blank: i32, _info: *mut c_void) -> i32 { 0 }
ksym!(drm_fb_helper_blank);
```

But each one corresponds to a different fbdev operation
(blank screen, validate var-info, ioctl dispatch, set color
map, etc.).  The signatures differ; the stubs all return 0.
For K15's purposes the difference is documentation only.

The same is true of the 5 `fb_*`, the 3 `ttm_bo_*`, the 5
`drm_client_*` — all sharing the link-only no-op pattern.
What's encoded in the file structure is the *eventual*
boundary: when K20+ a real driver calls `drm_fb_helper_blank`,
we know which file to edit because each subsystem lives in
its own module.

Mass-production of stubs has shape.

## The mutex / module_ref design choice

Two stubs of note in the "no-op now, real later" bucket:

**`mutex_lock` / `mutex_unlock`**: Linux's sleepable kernel
mutex with priority inheritance and adaptive spinning is
substantial code.  K15 stubs them to no-ops because every
K1-K15 module-load path runs in **single-threaded init
context** — there is no contention to serialize.  When K20+
has probe paths firing on real threads, we route to either a
`SpinLock` (cheap) or a real sleepable mutex implementation.

**`try_module_get` / `module_put`**: refcount Linux uses to
prevent unloading a module while a caller holds a reference.
Kevlar doesn't unload modules yet.  `try_module_get` always
succeeds; `module_put` is a no-op.  When module unloading
becomes a thing (likely K30+ for hot-add/hot-remove), we
add a real refcount.

Both are "the right shape for now, real later" — exactly the
LinuxKPI playbook.

## What didn't have to be done

- **Real fbdev / fbcon raster ops.**  `sys_copyarea`,
  `sys_fillrect`, `sys_imageblit` are no-ops.  K20+ when an
  actual fbcon surface tries to draw, we replace with real
  memcpy/memset patterns.
- **TTM buffer-object semantics.**  Stubs only; no real
  vmap, no real GPU memory model.
- **DRM format metadata.**  `drm_format_info_bpp` returns 0;
  callers will need real values when probe runs.
- **`struct drm_fb_helper` layout exactness.**  Nothing
  reads fields yet.

## Cumulative kABI surface (K1-K15)

~200 exported symbols (K14's 160 + 40 K15 net new).  44+ shim
modules in `kernel/kabi/`.  Three DRM helpers loadable.

## Status

| Surface | Status |
|---|---|
| K1-K14 | ✅ |
| K15 — drm_ttm_helper.ko + 40 fbdev/TTM stubs | ✅ |
| K16+ — drm_dma_helper.ko, then real DRM driver | ⏳ |

## What K16 looks like

drm_dma_helper.ko is the next rung — 79 undefs, but ~30 of
them overlap K15's surface (drm_fb_helper, fb_*, sys_raster,
mutex, module_ref, drm dbg, refcount).  Net new is ~30:

- **`dma_alloc_attrs`, `dma_alloc_pages`, `dma_free_attrs`,
  `dma_free_pages`, `dma_get_sgtable_attrs`, `dma_mmap_attrs`,
  `dma_mmap_pages`** — DMA coherent-allocation API.  Some
  overlap with K5's K-level dma stubs but Linux-7.0-shape
  signatures need new wrappers.
- **`dma_buf_vmap_unlocked`, `dma_buf_vunmap_unlocked`** —
  DMA-buf vmap.
- **`drm_atomic_helper_damage_iter_init/_next`** — atomic
  KMS damage iteration.
- **`drm_gem_create_mmap_offset`, `drm_gem_handle_create`,
  `drm_gem_object_init`, `drm_gem_object_release`,
  `drm_gem_prime_mmap`, `drm_gem_private_object_init`,
  `drm_gem_vm_open/close`** — full DRM GEM API.
- **`drm_mode_size_dumb`, `drm_prime_gem_destroy`,
  `drm_prime_get_contiguous_size`** — DRM modes / DRM prime.
- **`__dma_sync_single_for_device`, `__vma_start_write`,
  `is_vmalloc_addr`, `memstart_addr`, `vm_get_page_prot`** —
  arch / mm helpers.

K16 should be similar shape to K15 — mostly link-only stubs.
Maybe one file extension for K5's existing dma.rs.

After K16:
- **K17-K19**: catch-up + small DRM core stubs (drm.ko-equivalent
  surface) needed to attempt a real tiny driver
  (cirrus-qemu.ko or gm12u320.ko).
- **K20+**: a real DRM driver probes — the *callback* surface
  area starts to matter.  This is when K15's `mutex_lock`
  decision bites or doesn't, when `drm_fb_helper_set_par` is
  expected to actually configure something.

The "graphical ASAP" arc is now ~6-7 milestones away.  K15
was the big lift; K16 is the cleanup pass before the curve
flattens into "real semantics" territory.
