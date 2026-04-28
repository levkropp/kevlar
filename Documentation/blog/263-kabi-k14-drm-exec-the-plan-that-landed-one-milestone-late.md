# 263 — kABI K14: drm_exec.ko, the plan that landed one milestone late

K14 lands.  Ubuntu 26.04's `drm_exec.ko` — the DRM transactional
buffer-reservation helper — loads in Kevlar with all 11 symbols
resolved.

```
kabi: loading /lib/modules/drm_exec.ko (Ubuntu 26.04)
kabi: loaded /lib/modules/drm_exec.ko (15833 bytes, 32 sections, 93 symbols)
kabi: /lib/modules/drm_exec.ko license=Some("Dual MIT/GPL")
       author=None desc=Some("DRM execution context")
kabi: applied 86 relocations (7 trampoline(s))
kabi: drm_exec is a library module (no init_module)
```

`make ARCH=arm64 test-module-k14` is the new regression target.
15 kABI tests now pass.

## The plan that landed one milestone late

The original K13 plan predicted ww_mutex, dma_resv, drm_gem,
refcount stubs would be needed.  drm_buddy didn't use any of
those — its surface turned out to be slab + rbtree + list
debug.  We wrote the K13 stubs against drm_buddy's actual
shape and shipped.

But the discarded K13 draft wasn't wrong about the work — it
was wrong about *which module needs it*.  drm_exec.ko's undef
list is **exactly** the four-subsystem surface the misfired
K13 plan described:

```
ww_mutex (4):                        already-stubbed (2):
  ww_mutex_lock                        kvfree (K2)
  ww_mutex_lock_interruptible          alt_cb_patch_nops (K11)
  ww_mutex_unlock
  reservation_ww_class (static)

dma_resv (1):                        kvmalloc renames (2):
  dma_resv_reserve_fences              __kvmalloc_node_noprof
                                       kvrealloc_node_align_noprof

drm_gem (1):                         refcount (1):
  drm_gem_object_free                  refcount_warn_saturate
```

K14's implementation is the K13 draft, recycled almost
verbatim.  Four small files, ~80 lines total, plus a 20-line
extension to `alloc.rs`.

## The smallest milestone yet

drm_exec.ko is **15833 bytes** — the smallest module Kevlar has
loaded from Ubuntu's tree.  86 relocations applied, 7
trampolines emitted, all 11 symbols resolved.  The whole load
path completes in microseconds.

The story under the hood:

- **No `init_module`.**  drm_exec is a *pure* library — its
  exports (`drm_exec_init`, `drm_exec_lock_obj`, etc.) are
  consumed by other DRM drivers; nothing in drm_exec itself
  runs at module-load time.  Our loader resolves every symbol,
  applies every relocation, and then has nothing to call.
- **Two DRM-stack modules now coexist.**  drm_buddy and
  drm_exec load back-to-back; future modules can reference
  either one's `EXPORT_SYMBOL` surface.  (Today nothing does;
  K17+ will.)
- **The compounding payoff hits again.**  Of 11 undefs, 2 were
  already stubbed (kvfree from K2; alt_cb_patch_nops from
  K11).  The K12/K13 kmalloc-rename foundation absorbed
  another two indirectly (the `__kvmalloc_node_noprof` /
  `kvrealloc_node_align_noprof` extensions are 4-line
  wrappers over existing K2 routines).

## What the four new shim files actually contain

**`ww_mutex.rs`** — three no-op functions returning 0 / void
plus a 64-byte zeroed `reservation_ww_class` static.  Linux's
real wait-wound mutex is a deadlock-avoiding nestable lock with
an integer "ticket" stamping each acquire; K14 ignores all of
that because nothing actually contends in init context.

**`dma_resv.rs`** — one function returning 0.  DMA reservation
objects manage fence-based GPU/CPU sync; we don't have GPUs
running yet so the API is purely surface.

**`drm_gem.rs`** — one no-op function with the right signature.
`drm_gem_object_free` is a kref release callback; drm_exec
references it but never invokes it at load.

**`refcount.rs`** — one function that logs and returns.  Linux
saturates refcount overflows here rather than wrapping; ours
just logs.

The honest description of K14: **scaffolding that exists so
the linker is happy**.  Eight functions out of nine net-new
will not be called by drm_exec at any point during its
lifetime.  The ABI surface compiles; the implementation
underneath is whatever-suffices.

This is the right shape for K14 — when K20+ has a real DRM
driver invoking these stubs from probe(), we go back and put
real code under them.  Until then, the milestone gates are
"linker satisfied" + "no panics" + "K1-K13 still pass."

## The `kvrealloc_node_align_noprof` cheat

One stub deserves a footnote.  Linux's `kvrealloc` is the
`kv*` family's realloc — it has to handle three cases (heap
→ heap, vmalloc → vmalloc, heap ↔ vmalloc) because the
backing store can change with the new size.  Kevlar's K2
`kvmalloc` dispatches by size threshold but doesn't track
which backing each pointer used.

K14's stub punts: it just calls `krealloc`, which only knows
about the heap path.  drm_exec doesn't actually invoke this
at load — it's purely linker fodder — so the lie holds.  When
something *does* call this (likely K17 or K18), we'll need to
introduce a "where did this pointer come from" probe akin to
the existing vmalloc side-table.  Defer.

## What didn't have to be done

- **Real ww_mutex semantics.**  Single-threaded, no contention;
  no-op holds.
- **`struct ww_class` layout exactness.**  64-byte zero buffer.
  Real layout is non-trivial (mutex tracking + class name).
  K20+ when a caller dereferences it.
- **dma_resv fence machinery.**  We don't synchronize.
- **drm_gem object lifecycle.**  No objects exist; nothing
  allocates them.

## Cumulative kABI surface (K1-K14)

~160 exported symbols.  35+ shim modules in `kernel/kabi/`.  Two
DRM helpers loadable; the next ones in the queue
(drm_ttm_helper, drm_dma_helper) reference these K14 symbols
(ww_mutex, refcount_warn_saturate, drm_gem_object_free) so the
work compounds forward.

## Status

| Surface | Status |
|---|---|
| K1-K13 | ✅ |
| K14 — drm_exec.ko + ww_mutex/dma_resv/drm_gem/refcount | ✅ |
| K15+ — drm_ttm_helper.ko, drm_fb_helper, fb raster ops | ⏳ |

## What K15 looks like

drm_ttm_helper.ko is the next rung — 47 undefs, ~30 net new.
The big new surface area:

- **drm_fb_helper_** (12 funcs).  Framebuffer-emulation
  helpers DRM uses to expose a `/dev/fb0`-shaped char device
  on top of GPU buffers.  When K20+ runs a real DRM driver,
  this is what makes Linux fbcon / fbdev userspace work
  against it.
- **drm_client_** (4 funcs).  DRM client API for in-kernel
  consumers of DRM (fbcon, drm_fbdev_*).
- **fb_deferred_io_** (3 funcs) + **fb_sys_*** (2 funcs).
  Framebuffer-side primitives.
- **sys_copyarea / sys_fillrect / sys_imageblit**.  The
  software framebuffer raster ops — what fbcon uses to
  draw text on screen if the GPU doesn't accelerate.  These
  are nontrivial: they actually do work (memcpy + memset
  patterns over a framebuffer) and we may want real
  implementations rather than stubs since fbcon-on-Kevlar
  would be the first end-to-end test of the display path.

K15-K16 likely splits into two sessions: the helper stubs in
K15, then if any sys_* raster op needs to actually fire, real
implementations in K16.

After that the climb continues.  drm_dma_helper (79 undefs)
is K17ish.  By K20 we should have enough DRM core stubs to
attempt a real tiny driver (cirrus-qemu or gm12u320), which
is when probe() actually starts running and the ABI promise
becomes literal.

The "graphical ASAP" arc keeps ticking down.  K14 was the
one-night session; K15 is when DRM-stack work gets bigger
again.
