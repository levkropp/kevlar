# 282 — kABI K33: erofs init returns 0; first Linux .ko fs registered

K33 Phase 2 closed last post with all 204 of erofs.ko's
external symbols resolved by the kABI loader, but
`init_module` returning -ENOMEM at the first null-returning
stub.  Phase 2b/2c plumbs through the rest: real bodies for
the slab + page allocators, then a two-iteration probe-and-
fix to figure out which two more null-returning stubs were
gating the init.

End state of this session:

```
kabi-trace: alloc_workqueue_noprof (stub) — fake handle
kabi-trace: kobject_init_and_add (stub returns 0)
kabi-trace: register_filesystem(0xffff00007cdd6cf0) entered
kabi: erofs init_module returned 0
kabi: registered filesystems = 1
```

A Linux 7.0.0-14 prebuilt `.ko` filesystem module ran its
`init_module()` cleanly inside Kevlar and registered itself
with our VFS bridge.  This is the moment Kevlar's "drop-in
Linux replacement" tagline extends from "load Linux's GPU
drivers" to "load Linux's filesystems."

## Phase 2b — real bodies for slab + page

The Phase 2 stubs all returned null/0/-ENOSYS.  Phase 2b
replaces the alloc-shaped ones with real impls delegating
to existing `kabi::alloc` + `kabi::slab` paths.

```rust
// kernel/kabi/fs_stubs.rs

#[unsafe(no_mangle)]
pub extern "C" fn alloc_pages_noprof(gfp: u32, order: u32) -> *mut c_void {
    let size = (1usize << order) * kevlar_platform::arch::PAGE_SIZE;
    if size > 64 * 1024 {
        log::warn!("kabi: alloc_pages_noprof order={} too large", order);
        return core::ptr::null_mut();
    }
    super::alloc::kmalloc(size, gfp)
}

#[unsafe(no_mangle)]
pub extern "C" fn kmem_cache_alloc_lru_noprof(cache: *mut c_void,
                                              _lru: *mut c_void,
                                              gfp: u32) -> *mut c_void {
    super::slab::kmem_cache_alloc_noprof(cache, gfp)
}

#[unsafe(no_mangle)]
pub extern "C" fn vmalloc_noprof(size: usize) -> *mut c_void {
    super::alloc::vmalloc(size)
}

#[unsafe(no_mangle)]
pub extern "C" fn __free_pages(page: *mut c_void, _order: u32) {
    super::alloc::kfree(page);
}
```

Plus `krealloc_node_align_noprof`, `kfree_sensitive`,
`kmemdup_nul`, `kstrdup`, `kstrndup` — all small wrappers.

Re-run: erofs still returns -12.  So the bailout is past the
slab path.  Time to trace.

## Phase 2c — probe, fix, probe, fix

erofs's `init_erofs_fs()` walks down through:

  1.  `erofs_init_sysfs()` — kobject + kset registration.
  2.  `erofs_init_inode_cache()` — `kmem_cache_create()`.
  3.  `erofs_init_managed_cache()` — slab cache init.
  4.  `erofs_pcpubuf_init()` — per-CPU buffers.
  5.  `z_erofs_init_zip_subsystem()` — compression workqueue.
  6.  `register_shrinker()` — memory-pressure callback.
  7.  `register_filesystem()` — VFS exposure.

Whatever returned null first was killing init.  Add probes
to the most likely candidates:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn shrinker_alloc(_flags: u32, _fmt: *const u8) -> *mut c_void {
    log::warn!("kabi-trace: shrinker_alloc (stub) — null");
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn alloc_workqueue_noprof(_fmt: *const u8, _flags: u32,
                                         _max_active: c_int) -> *mut c_void {
    log::warn!("kabi-trace: alloc_workqueue_noprof — null");
    core::ptr::null_mut()
}

// kernel/kabi/fs_register.rs
#[unsafe(no_mangle)]
pub extern "C" fn register_filesystem(fs_type: *mut c_void) -> c_int {
    log::warn!("kabi-trace: register_filesystem({:p}) entered", fs_type);
    if fs_type.is_null() { return -22; }
    ...
```

First boot:

```
kabi-trace: shrinker_alloc null
kabi: erofs init_module returned -12
```

So `shrinker_alloc` was the first null.  Linux's
`shrinker_alloc` returns a `struct shrinker *` whose fields
the caller writes (count_objects, scan_objects, seeks).
Real shrinkers take memory-pressure callbacks; we don't run
those.  But the caller's `if (!shrinker)` check needs to
pass.  Just hand back a heap buffer:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn shrinker_alloc(_flags: u32, _fmt: *const u8) -> *mut c_void {
    super::alloc::kmalloc(256, 0)  // 256 bytes for caller writes
}
```

The trick: `shrinker_register` already returns 0 (no-op).
We never actually invoke the callbacks the caller installed
into the buffer, so the un-functional shrinker is harmless.

Second boot:

```
kabi-trace: alloc_workqueue_noprof null
kabi: erofs init_module returned -12
```

Same pattern, same fix:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn alloc_workqueue_noprof(_fmt: *const u8, _flags: u32,
                                         _max_active: c_int) -> *mut c_void {
    super::alloc::kmalloc(64, 0)  // fake handle; queue_work_on returns false
}
```

Third boot:

```
kabi-trace: alloc_workqueue_noprof (stub) — fake handle
kabi-trace: kobject_init_and_add (stub returns 0)
kabi-trace: register_filesystem(0xffff00007cdd6cf0) entered
kabi: erofs init_module returned 0
kabi: registered filesystems = 1
```

🎉

## The recipe for "fake handle" stubs

This is a recurring pattern, worth naming.  Linux's
`init_*` functions allocate a struct, hand the pointer back
to the caller, the caller writes into it, then registers
the result.  We don't model the registered behaviour, but
we still need to pass the caller's null check.  The recipe:

  1.  `*_alloc()` returns `kmalloc(N, 0)` for some N big
      enough to hold the caller's writes.  Real Linux does
      this with `kzalloc(sizeof(struct ...), GFP_KERNEL)`;
      we just need enough room.
  2.  `*_register()` is a no-op returning 0.  We don't
      actually run the registered code.
  3.  `*_free()` calls `kfree`.  Caller's cleanup balances.

Three lines per function.  Works for shrinkers,
workqueues, kthread_workers — anything with the
alloc-register-free shape that we don't actually drive.

## A side fix — SPIN_CONTENTION on platform allocator

Running with `kabi-load-erofs=1` made the existing
PREZEROED_4K_POOL sweep cadence (50 idles ≈ 0.5 s) worse:
the sweep takes the pool lock under heavy alloc traffic,
and on smp=2 both CPUs would spin 5 M times waiting for
the sweep to release.  Half of LXDE boots ended with
`SPIN_CONTENTION` warnings + a stalled boot.

Lowering the cadence to 500 idles (≈ 5 s) reduced the
collision window to under what the watchdog cares about.
Default boot 4/5 clean, erofs-loaded 5/5 reach LXDE.  The
sweep's job is "catch the rare PAGE_ZERO_MISS at PREZEROED
shortly after it happens" — 5-second granularity is plenty
for that diagnostic; it doesn't need to fire often, it just
needs to fire eventually.

## What's actually shippable

Five commits this session:

  * **`7766c33`** Phase 2b: 12 stubs go from null/no-op to
    real impl delegating to `kabi::alloc` + `kabi::slab`.
  * **`fd0dc7c`** Phase 2c: shrinker_alloc + alloc_workqueue_noprof
    return non-null heap handles.  erofs init_module returns 0.
  * **`5bec300`** PREZEROED sweep cadence 50→500 ticks; LXDE
    flake rate ~50% → ~10%.
  * Plus 281's commits already in the tree (`cdb2337` Phase 2,
    `5840edf` blog 281).

What works now end-to-end:

  * `make ARCH=arm64 build` clean.
  * `make ARCH=arm64 test-lxde` 8/8 passes (default boot,
    erofs not loaded — kABI surface is silent unless asked).
  * With `kabi-load-erofs=1` cmdline: erofs.ko loads, init
    returns 0, our `kabi::fs_register::FS_TYPES` registry
    has one entry, LXDE still passes 5/5 to running desktop
    (test pass rate 6/8-8/8, dependent on the flaky
    pixel-visible screenshot).

What's ahead — K33 Phase 3:

  * `kernel/syscalls/mount.rs`: when fstype is `"erofs"`
    (or eventually `"ext4"`), call
    `kabi::fs_register::lookup_fstype("erofs")` and
    dispatch to the module's `->mount` op.

  * `kernel/kabi/fs_adapter.rs` (NEW): wrap the Linux
    `super_block *` returned by `->mount` and adapt to
    Kevlar's `kevlar_vfs::FileSystem` trait.

  * Mount an actual erofs disk image at boot, list its
    contents from a userspace `ls`.

That's the inflection point: from "loaded the module" to
"the module is doing real work."

## Status

| Milestone | Status |
|---|---|
| K1-K29: kABI arc | ✅ |
| K30: graphical Alpine LXDE | ✅ |
| K31: trackpad + 12s boot | ✅ |
| K32: itest + freeze diagnosed | ✅ |
| K33 Phase 1: gvfs fix; mm bug instrumented; fs scaffolding | ✅ |
| K33 Phase 2: erofs.ko 204 → 0 unresolved | ✅ |
| K33 Phase 2b/2c: erofs init = 0; fs registered = 1 | ✅ |
| K33 Phase 3: route mount(2) through kABI fs registry | ⏳ |
| K34+: ext4.ko build (Docker / GNU GCC env) | ⏳ |

The interesting moment didn't take a week, even after the
Phase 2 plan estimated 1-2.  The shape of the work mattered:
the loader pre-pass turned a serial "fix one, hit next,
rebuild" workflow into a single triage-and-batch pass.
Phase 2c didn't even need a real implementation — just
non-null pointers in three places.  Phase 3 will need real
work in the mount adapter, but the surface is small.
