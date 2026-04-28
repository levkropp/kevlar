# 262 — kABI K13: drm_buddy.ko and the start of the display ascent

K13 lands.  Ubuntu 26.04's `drm_buddy.ko` — the buddy-allocator
helper that DRM drivers use to track GPU memory regions —
loads in Kevlar and `init_module` returns 0.

```
kabi: loading /lib/modules/drm_buddy.ko (Ubuntu 26.04)
kabi: loaded /lib/modules/drm_buddy.ko (36657 bytes, 41 sections, 184 symbols)
kabi: /lib/modules/drm_buddy.ko license=Some("Dual MIT/GPL")
       author=None desc=Some("DRM Buddy Allocator")
kabi: image layout: 21168 bytes (6 pages) at 0xffff00007d240000
kabi: applied 258 relocations (15 trampoline(s))
kabi: drm_buddy init_module returned 0
```

`make ARCH=arm64 test-module-k13` is the new regression target.
14 kABI tests now pass.

## A pivot from the K12 prediction

The K12 blog ended with a different K13 plan: real virtio bus
walking + struct layout exactness so virtio_input's probe could
fire.  That's still a milestone — but graphical Alpine needs
*both* input *and* display, and the display half had nothing
in place at all.  K13 instead starts the **DRM stack ascent**:
pick the smallest DRM `.ko` in Ubuntu's tree, satisfy its
linker, get it loading.  When K13 + a few peers are in place,
we go back and finish the virtio probe story.

`drm_buddy.ko` was the natural first target — 37 KB, 21
undefined symbols, none of which need ww_mutex / dma_resv /
drm_gem (the assumption the plan started with).  The actual
shape:

```
slab (kmem_cache) (4):
  __kmem_cache_create_args
  kmem_cache_alloc_noprof
  kmem_cache_destroy
  kmem_cache_free

red-black tree (5):
  rb_erase
  rb_first_postorder
  rb_insert_color
  rb_next_postorder
  rb_prev

list debug (2):
  __list_add_valid_or_report
  __list_del_entry_valid_or_report

DRM print (1):
  drm_printf

popcount fallback (1):
  __sw_hweight64

already-stubbed (8):
  __kmalloc_cache_noprof    (K12)
  __kmalloc_noprof          (K12)
  __stack_chk_fail          (K11)
  __ubsan_handle_shift_*    (K12)
  dynamic_cond_resched      (K11)
  kfree                     (K2)
  kmalloc_caches            (K12)
  random_kmalloc_seed       (K12)
```

Net new this milestone: **13 stubs across 5 new files**
(`slab.rs`, `rbtree.rs`, `list.rs`, `drm.rs`, `bitops.rs`).
8 of the 21 were already covered by K11/K12's groundwork.

## init_module on first run

Two milestones in a row that worked on the first build:

```
   8:  d503201f   nop
   c:  d503201f   nop
  10:  d503233f   paciasp
  14:  d10103ff   sub  sp, sp, #0x40
  18:  f800865e   str  x30, [x18], #8     ; SCS push
  ...
  64:  94000000   bl   <__kmem_cache_create_args>
  68:  f100001f   cmp  x0, #0
```

`drm_buddy` is "library-shaped" — it doesn't register a driver
or open a device — but its `init_module` does one tiny piece
of real work: creates a slab cache for `struct drm_buddy_block`
via `__kmem_cache_create_args`.  Our stub returns a small heap
header that carries the size, the comparison `cmp x0, #0`
succeeds (non-null), and init returns 0.

The slab cache stub deserves a moment.  Linux's slab caches
are size-class allocators with per-CPU magazines for hot paths;
our K13 version is **a 16-byte heap struct that just stores
`object_size`, plus three trivial wrappers**:

```rust
pub extern "C" fn __kmem_cache_create_args(
    _name: *const c_char, object_size: u32, ...,
) -> *mut c_void {
    let cache = kmalloc(size_of::<KmemCacheStub>(), 0)
                   as *mut KmemCacheStub;
    (*cache).object_size = object_size as usize;
    cache as *mut c_void
}

pub extern "C" fn kmem_cache_alloc_noprof(cache, gfp) -> *mut c_void {
    let size = (*(cache as *mut KmemCacheStub)).object_size;
    kmalloc(size, gfp)
}

pub extern "C" fn kmem_cache_free(_cache, ptr) { kfree(ptr); }
```

That's enough to make every Linux subsystem that "uses slab"
work correctly through a `kmalloc`/`kfree` underbelly.  The
ABI surface — `struct kmem_cache *` opaque, `_args` /
`_alloc_noprof` / `_free` / `_destroy` — is preserved, but
the implementation collapses to "kmalloc carrying the size."
Plenty of future modules will hit this path.

## The compounding pattern, again

K12's blog called out the compounding payoff: each detour you
absorb means the next driver doesn't see it.  K13 makes that
literal — three of the five new shim files don't fire at load
time at all.  `rbtree.rs` and `list.rs` are pure linker satisfy:
the symbols resolve, the relocations apply, the functions never
get called.  drm_buddy's *clients* — future DRM drivers calling
into the buddy API — are what would actually invoke them.  So
K13's stubs are scaffolding for K14+ rather than active code.

This is the same pattern as virtio_input: `init_module` does
one small thing (register), and the bulk of the symbol surface
is for callbacks that fire when a real device or caller arrives.

## What K13 didn't do

- **Real RB-tree / list / slab semantics.**  Stubs are no-op
  / null / "trust the caller."  When K14+ surfaces a DRM
  driver that exercises drm_buddy's allocator, RB-tree
  becomes load-bearing and we'll need real code.
- **Layout exactness for `struct drm_buddy_block`,
  `struct drm_buddy_mm`.**  drm_buddy returns these to its
  callers; nothing in K13 dereferences them.
- **DRM core (`drm.ko`).**  Built into Ubuntu's kernel
  (`=y`), not a loadable module.  When K15+ wants a real
  DRM driver to load, we have to replicate enough of
  drm.ko's exports as kABI stubs.
- **Virtio bus walking** (the original K13 plan).  Deferred
  to K20-ish.

## Cumulative kABI surface (K1-K13)

~151 exported symbols (K12's 138 + 13 K13 net new).  Five new
shim modules joining the 30+ already in `kernel/kabi/`.

## Status

| Surface | Status |
|---|---|
| K1-K12 | ✅ |
| K13 — drm_buddy.ko + slab/rbtree/list/drm/bitops | ✅ |
| K14+ — drm_exec.ko, then heavier DRM helpers | ⏳ |

## What K14 looks like

After K13 the natural next step is `drm_exec.ko` — another
small DRM helper (16 KB) for transactional buffer reservation.
Its undef list overlaps heavily with K13's: ww_mutex enters
the picture (the actual K13-plan target that turned out to
not be needed for drm_buddy), plus a handful of DRM core
symbols.  Expected shape: small session, mostly leveraging
K13's slab + list + drm_printf foundation.

After that the curve gets steeper: `drm_ttm_helper.ko` (~48
undefs), `drm_dma_helper.ko` (~79 undefs).  By K17 we should
have enough DRM helper coverage that picking up a *real*
display driver — `cirrus-qemu.ko` or similar — becomes
tractable.

The "graphical ASAP" arc is now ~9 milestones away.  K13 was
the on-ramp.
