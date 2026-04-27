# 261 — kABI K12: virtio_input.ko, two subsystems, no detours

K12 lands.  Ubuntu 26.04's `virtio_input.ko` — the kernel module
that connects QEMU's emulated keyboard and mouse to Linux's
input subsystem — loads in Kevlar and `init_module` returns 0.

```
kabi: loading /lib/modules/virtio_input.ko (Ubuntu 26.04)
kabi: loaded /lib/modules/virtio_input.ko (23681 bytes, 39 sections, 103 symbols)
kabi: /lib/modules/virtio_input.ko license=Some("GPL")
       author=Some("Gerd Hoffmann <kraxel@redhat.com>")
       desc=Some("Virtio input device driver")
kabi: __register_virtio_driver (stub)
kabi: virtio_input init_module returned 0
```

Two firsts:

1. **Two subsystems' worth of stubs in one milestone** — input
   core + virtio bus core, plus assorted infrastructure.  K11
   was one subsystem (network).  K12 was two.
2. **K12 worked on first run.**  No HVF asserts, no x18-clobber
   page faults, no kmalloc-name mismatches.  The K10/K11 detours
   absorbed every calling-convention concern transparently.
   30 undefined symbols, all 30 resolved on the first build,
   init_module returned 0.

`make ARCH=arm64 test-module-k12` is the new regression target.
13 kABI tests now pass.

## Why this milestone matters

The K11 blog ended with a prediction:

> Each will be a similar shape: load a module, hit a confusing
> fault, find the ABI assumption that diverges, fix at the kernel
> layer, every subsequent module benefits.

K12 is the first milestone since K9 where we *didn't* hit a
confusing fault.  The compounding payoff of the LinuxKPI
playbook is starting to be measurable: each detour you absorb
into the kernel layer means the next driver doesn't see it.

What absorbed K12's potential detours?

- **K10's `LoadedModule::call_init()`** — sets up x18 to a
  valid SCS area before calling init.  Every Ubuntu module
  uses SCS.
- **K11's `-Z fixed-x18`** — Rust kernel never clobbers x18
  inside our shims.  Every module that calls back into shims
  benefits.
- **K6's variadic printk** — virtio_input uses `_dev_err(dev,
  "...", args)` which we shimmed by reusing the K6 formatter.
- **K10's `cpu_have_feature`** — surfaced from xor-neon;
  reused by `__register_virtio_driver`'s feature check.

By the time K12 attempted virtio_input, every cross-cutting
issue from K1-K11 had been absorbed.  Only the
*subsystem-specific* surface (input + virtio core) remained.

## The 30 undefs, classified

```
input subsystem (8):
  input_alloc_absinfo
  input_allocate_device
  input_event
  input_free_device
  input_mt_init_slots
  input_register_device
  input_set_abs_params
  input_unregister_device

virtio core (9):
  __register_virtio_driver
  unregister_virtio_driver
  virtio_reset_device
  virtqueue_add_inbuf_cache_clean
  virtqueue_add_outbuf
  virtqueue_detach_unused_buf
  virtqueue_get_buf
  virtqueue_get_vring_size
  virtqueue_kick

kmalloc renames (3 funcs + 2 statics):
  __kmalloc_cache_noprof  (alias to kmalloc)
  __kmalloc_noprof        (alias to kmalloc)
  random_kmalloc_seed     (static u64)
  kmalloc_caches          (static [u8; 4096])

spinlock primitives (2):
  _raw_spin_lock_irqsave
  _raw_spin_unlock_irqrestore

ubsan handlers (2):
  __ubsan_handle_load_invalid_value
  __ubsan_handle_out_of_bounds

scheduler (1):
  dynamic_might_resched

format string + reporting (2):
  snprintf
  _dev_err

scatterlist (1):
  sg_init_one

already-stubbed (2):
  __stack_chk_fail (K11)
  kfree (K2)
```

5 new files, 4 extended.  ~470 LOC across the diff.  Mechanical
work — no design beyond "satisfy the linker, return success."

## init_module's hot path

Disassembly tells the load-time story:

```
0000000000000008 <init_module>:
   8:  d503201f   nop
   c:  d503201f   nop
  10:  d503233f   paciasp
  14:  f800865e   str  x30, [x18], #8     ; SCS push
  18:  90000001   adrp x1, ...            ; arg1 = THIS_MODULE
  1c:  a9bf7bfd   stp  x29, x30, ...      ; standard frame
  20:  91000021   add  x1, x1, #0x0
  24:  90000000   adrp x0, ...            ; arg0 = &virtio_input_driver
  28:  910003fd   mov  x29, sp
  2c:  91000000   add  x0, x0, #0x0
  30:  94000000   bl   <__register_virtio_driver>
  34:  f85f8e5e   ldr  x30, [x18, #-8]!   ; SCS pop
  38:  f84107fd   ldr  x29, [sp], #16
  3c:  d50323bf   autiasp
  40:  d2800001   mov  x1, #0
  44:  d2800010   mov  x16, #0
  48:  d2800011   mov  x17, #0
  4c:  d65f03c0   ret
```

A single function call: `__register_virtio_driver(&driver,
THIS_MODULE)`.  Our stub logs and returns 0.  The other 29
undefs are referenced from probe/remove paths that fire only
when the bus matches the driver to a real virtio device — and
in K12 nothing does.

## What didn't have to be done

`__register_virtio_driver` is *registration*, not *probing*.
Linux's real implementation appends the driver to `virtio_bus`'s
driver list, then walks the list of pending devices and calls
the matching driver's `probe()` for each match.  Our stub
records nothing.

This means probe **doesn't fire**.  None of virtio_input's
callbacks (`virtinput_probe`, `virtinput_remove`, etc.) execute.
The 21 input/virtio symbols those callbacks reference are
linker-only — present at the time of relocation, never invoked.

K13 changes this: real virtio bus walking + struct layout
exactness so probe can run successfully.  At that point,
`/dev/input/eventN` becomes a real thing and Xorg-class
software can read keystrokes.  But K12 stops short — get
the *load* working; defer the *bus binding* to K13.

## The kmalloc-rename trap that didn't trip

Linux 7.0's MM subsystem renamed several allocator entry points
to `_noprof` variants for memory-profiling support
(`CONFIG_MEM_ALLOC_PROFILING`).  Modules built against 7.0
headers reference:

- `__kmalloc_noprof` instead of plain `__kmalloc`
- `__kmalloc_cache_noprof` instead of size-class kmalloc
- `kvmalloc_node_noprof` instead of `kvmalloc_node`
- `kmemdup_noprof` instead of `kmemdup`

K12 added explicit aliases for all four, plus `devm_kmalloc` /
`devm_kmemdup` (device-managed allocations — Linux frees on
device removal; ours leaks).  Each is a one-line wrapper over
our existing K2 `kmalloc`/`kvmalloc`.

```rust
#[unsafe(no_mangle)]
pub extern "C" fn __kmalloc_noprof(size: usize, gfp: u32) -> *mut c_void {
    kmalloc(size, gfp)
}
ksym!(__kmalloc_noprof);
```

Without these, virtio_input would have failed to load with a
missing-`__kmalloc_noprof` error.  Instead — instant resolution.
The compounding payoff again: every future module that
allocates memory inherits the rename aliases, which is *every
non-trivial module*.

## What K12 didn't do

- **Real virtio bus walking.**  `__register_virtio_driver`
  records the registration but doesn't walk pending devices.
  Future K13: real bus + probe firing.
- **Layout exactness for `struct input_dev` and `struct
  virtqueue`.**  Our stubs return zeroed buffers.  Once probe
  runs and reads fields off these structs, layouts matter.
- **`/dev/input/eventN` visible to userspace.**  Linux's input
  layer creates a char device in the input class.  K12 doesn't
  expose this — `input_register_device` is a no-op.
- **MTE / PAC-key enablement.**  Still off.  paciasp /
  autiasp run as nops.

## Cumulative kABI surface (K1-K12)

138 exported symbols (K11's 108 + ~30 K12 additions).  Plus the
two infrastructure pieces from K10/K11 (SCS-aware `call_init`
wrapper; `-Z fixed-x18` build flag).

## Status

| Surface | Status |
|---|---|
| K1-K11 | ✅ |
| K12 — virtio_input.ko + input + virtio core | ✅ |
| K13+ — layout exactness, real bus probing, fbdev, DRM | ⏳ |

## What K13 looks like

K13 is the first milestone where we make a loaded Ubuntu
module's *callback* fire.  Two intertwined pieces:

1. **Real virtio bus walking.**  Track registered drivers in a
   list.  Provide a way to "fake-register" a virtio device
   (probably statically declared in `kernel/kabi/virtio.rs`
   for K13; later from QEMU's actual MMIO virtio devices).
   When a driver registers, walk pending devices; on match
   call probe.
2. **Struct layout exactness for `struct virtqueue` + `struct
   virtio_device`.**  When virtio_input's probe reads
   `vdev->config->find_vqs(...)` or `vdev->dev.parent`, those
   field offsets need to match Linux's exact layouts.

If K13 succeeds, virtio_input.ko's probe runs against a
fake/real virtio-input device, calls `input_register_device`,
and its event-pump path is wired up.  We may not yet have
keystrokes flowing to userspace, but the driver has gone from
"loaded" to "instantiated" — a meaningful step toward
graphical Alpine.

K13 is also where the *callback* surface area starts to
matter.  K11/K12 only validated linker-side resolution.  K13
validates that **a real Ubuntu kernel module can call back
into Kevlar** through fields we control the layout of.  That's
the ABI promise becoming literal.
