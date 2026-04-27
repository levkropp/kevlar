# 254 — kABI K5: ioremap, MMIO, dma_alloc_coherent

K5 lands.  Loaded `.ko` modules can now allocate DMA-coherent
buffers, `ioremap` physical addresses, and perform memory-
mapped I/O via `readl`/`writel`.  After K5, every standard
Linux device-driver init pattern is expressible:

```
register on a bus → probe → ioremap → dma_alloc_coherent → MMIO
```

The proof, on serial:

```
kabi: loading /lib/modules/k5.ko
kabi: /lib/modules/k5.ko license=Some("GPL") author=Some("Kevlar")
       desc=Some("kABI K5 demo: ioremap + readl/writel + dma_alloc_coherent")
[mod] [k5] init begin
[mod] [k5] dma_alloc_coherent ok
[mod] [k5] ioremap ok
[mod] [k5] writel ok (buf reads 0xCAFEBABE)
[mod] [k5] readl ok (io reads 0xDEADBEEF)
[mod] [k5] phys/virt round-trip ok
[mod] [k5] init done
kabi: k5 init_module returned 0
```

`make ARCH=arm64 test-module-k5` is the regression target.
All five kABI tests now pass: K1 (loader), K2 (alloc/wait/work),
K3 (device model), K4 (file_operations), K5 (MMIO/DMA).

## What the demo does

A self-contained verification of every K5 primitive without
needing real hardware:

```c
int init_module(void) {
    dma_addr_t dma_pa = 0;
    void *buf = dma_alloc_coherent(NULL, 4096, &dma_pa, 0);

    /* Map the same physical address through ioremap to get a
     * second kernel VA.  Both pointers must reach the same bytes. */
    void *io = ioremap(dma_pa, 4096);

    writel(0xCAFEBABE, io);
    if (*(volatile unsigned int *)buf != 0xCAFEBABE) return -1;

    *(volatile unsigned int *)buf = 0xDEADBEEF;
    if (readl(io) != 0xDEADBEEF) return -1;

    iounmap(io);
    dma_free_coherent(NULL, 4096, buf, dma_pa);
    return 0;
}
```

Two pointers (`buf` from DMA, `io` from ioremap), two
write paths (writel + direct dereference), two read paths
(readl + direct dereference), four cross-checks.  Either
both views see the same memory or one of them fails.

## The architecture is mostly "wrap what's already there"

K5's kernel side is unusually thin — about 200 lines total
across `kernel/kabi/io.rs` and `kernel/kabi/dma.rs`.

`readl(addr)`:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn readl(addr: *const c_void) -> u32 {
    if addr.is_null() { return 0; }
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}
```

`writel`, `readb`, `readq`, `writeq` etc. are the same
shape with different element types.  Linux's `readl(addr)`
*is* a volatile load; Kevlar's is too.

`ioremap(phys, size)`:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn ioremap(phys: u64, _size: usize) -> *mut c_void {
    let pa = PAddr::new(phys as usize);
    pa.as_vaddr().value() as *mut c_void
}
```

`PAddr::as_vaddr()` is `phys + KERNEL_BASE_ADDR` — Kevlar's
existing kernel direct map covers the entire QEMU virt RAM
range (~4 GB).  Any physical address in that range is
already reachable from a kernel VA; ioremap just returns
that VA.

This works because K5's intended use case is *DMA-coherent
buffers in main RAM*, not real PCI BARs.  PCI BARs sit at
high physical addresses (≥ 4 GB on QEMU virt arm64) and
need real page-table allocation in the vmalloc area; that's
K6 work, gated by simplefb.ko or whichever first binary
module needs it.

`iounmap` is a no-op — the direct map is permanent.

`dma_alloc_coherent`:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn dma_alloc_coherent(
    _dev: *mut DeviceShim,
    size: usize,
    dma_handle_out: *mut u64,
    _gfp: u32,
) -> *mut c_void {
    let num_pages = align_up(size, PAGE_SIZE) / PAGE_SIZE;
    let pa = match alloc_pages(num_pages, AllocPageFlags::KERNEL) {
        Ok(p) => p,
        Err(_) => return core::ptr::null_mut(),
    };
    let va = pa.as_vaddr().value();
    unsafe { core::ptr::write_bytes(va as *mut u8, 0, num_pages * PAGE_SIZE); }
    if !dma_handle_out.is_null() {
        unsafe { *dma_handle_out = pa.value() as u64; }
    }
    va as *mut c_void
}
```

That's `alloc_pages` (Kevlar's existing buddy allocator)
plus zero-fill plus writing the PA back through the
caller's out-parameter.  The buddy allocator returns
physically-contiguous chunks; arm64 QEMU virt is cache-
coherent for PCI; Linux's "DMA address" is just the
physical address on this platform.

## arm64 cache coherency: the no-op map_single

Real Linux's `dma_map_single(dev, ptr, size, DMA_TO_DEVICE)`
flushes CPU caches so a device DMA-reading the buffer sees
the latest values.  `dma_unmap_single(..., DMA_FROM_DEVICE)`
invalidates caches so the CPU sees what the device wrote.

On QEMU virt arm64, PCI devices snoop the CPU cache.  The
flush/invalidate ops are unnecessary — the cache is already
coherent.  K5's `dma_map_single` and `dma_unmap_single`
collapse to no-ops:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn dma_map_single(
    _dev: *mut DeviceShim, ptr: *mut c_void,
    _size: usize, _dir: i32,
) -> u64 {
    let va = VAddr::new(ptr as usize);
    va.as_paddr().value() as u64
}

#[unsafe(no_mangle)]
pub extern "C" fn dma_unmap_single(...) {
    // No-op on cache-coherent arm64.
}
```

A real ARM SoC (Raspberry Pi, an actual silicon device)
would need `dc civac` cache maintenance ops in here.  When
that platform target lands, K5's `dma_map_single` grows the
appropriate `cache_flush_range(va, size)` call.  Until
then, the no-op is correct for the only platform Kevlar
runs on today.

## What the K5 demo proves

The demo exercises a property real drivers depend on: that
two pointer-views into the same physical memory (one from
the allocator, one from `ioremap`) see the same bytes.

```
1. dma_alloc_coherent → buf=VA1, dma_pa=PA
2. ioremap(dma_pa)    → io=VA2  (different VA, same PA)
3. writel(0xCAFEBABE, io)
4. *(buf) == 0xCAFEBABE   ← if false, ioremap returned wrong VA
5. *buf = 0xDEADBEEF
6. readl(io) == 0xDEADBEEF  ← if false, MMIO accessor wrong
```

A real network driver calls `pci_alloc_coherent` for a ring
buffer, and the device DMA-writes packet data into it; the
driver reads the buffer through its kernel pointer.  The
relationship is the same: two views, same memory.  K5's
test catches any divergence between the two VA→PA paths.

On Kevlar, both VAs end up pointing to the same direct-map
slot — `VA1 == VA2` actually, because we collapsed both
paths onto `pa.as_vaddr()`.  That's a simplification real
Linux doesn't share (Linux's ioremap goes through a separate
vmalloc-area mapping, so VA1 ≠ VA2 even though they point
to the same physical bytes).  K6's real ioremap will
distinguish them.

## What K5 didn't do

- **Real ioremap with page-table mapping.**  K5 only works
  for physical addresses in the direct map.  K6 adds the
  vmalloc-area-allocating version when simplefb.ko needs
  to map a framebuffer outside main RAM.
- **Real non-contiguous `vmalloc`.**  K2's
  alloc_pages-based vmalloc handles allocations up to the
  buddy max chunk; non-contiguous stitching defers to
  whenever a 100 MB+ allocation surfaces.
- **scatter-gather DMA** (`sg_table`, `dma_map_sg`).
- **Real cache flush ops** for non-coherent platforms.
- **`memcpy_fromio` / `memcpy_toio`.**  Multi-byte MMIO
  copies; defer until a driver needs them.
- **`pci_iomap` + PCI BAR-based ioremap.**  Deferred with
  the rest of PCI bus support.
- **Real `copy_to_user` against userspace addresses.**
  K4's memcpy still suffices for kernel-context tests.
- **Linux struct-layout exactness.**  Still K6.

## Cumulative kABI surface (K1-K5)

```
printk
kmalloc kzalloc kcalloc krealloc kfree
vmalloc vzalloc vfree kvmalloc kvzalloc kvfree
init_waitqueue_head destroy_waitqueue_head
wake_up wake_up_all wake_up_interruptible wake_up_interruptible_all
kabi_wait_event
init_completion destroy_completion
complete complete_all wait_for_completion
kabi_init_work schedule_work flush_work cancel_work_sync
kabi_current kabi_current_pid kabi_current_comm
msleep schedule cond_resched schedule_timeout
kref_init kref_get kref_put kref_read
kobject_init kobject_get kobject_put kobject_set_name kobject_add kobject_del
device_initialize device_add device_register device_unregister
get_device put_device dev_set_drvdata dev_get_drvdata
driver_register driver_unregister bus_register bus_unregister
platform_device_register platform_device_unregister
platform_driver_register platform_driver_unregister
__platform_driver_register
platform_set_drvdata platform_get_drvdata
platform_bus_type
alloc_chrdev_region register_chrdev_region unregister_chrdev_region
cdev_init cdev_add cdev_del
register_chrdev unregister_chrdev
copy_to_user copy_from_user clear_user strnlen_user
readb readw readl readq writeb writew writel writeq
ioremap ioremap_wc ioremap_nocache ioremap_cache iounmap
dma_alloc_coherent dma_free_coherent
dma_map_single dma_unmap_single
virt_to_phys phys_to_virt
```

~85 symbols.  Linear scan in the symbol resolver still
sub-microsecond at this scale.

## Status

| Surface | Status |
|---|---|
| K1 — ELF .ko loader | ✅ |
| K2 — kmalloc / wait / work / completion | ✅ |
| K3 — device model + platform bind/probe | ✅ |
| K4 — file_operations + char-device | ✅ |
| K5 — ioremap + MMIO + DMA | ✅ |
| K6 — load prebuilt Linux simplefb.ko | ⏳ next |
| K7-K9 | ⏳ |

## What K6 looks like

K6 is the inflection point.  K1-K5 built the runway: the
loader, the runtime, the device model, char-devices, and
I/O primitives.  *Everything has been Kevlar-shape demos*
— modules we wrote in `testing/k*-module.c` against headers
we control.

K6 takes a binary `.ko` from the Linux 7.0 source tree —
unmodified — and loads it.

The first target is `simplefb.ko`: a small DRM framebuffer
driver that takes a "simple-framebuffer" device tree node,
ioremap's the framebuffer's physical address, and exposes
it as `/dev/fb0`.  It's the smallest real Linux module
that exercises K1-K5 in concert.

What K6 needs:

1. **Linux struct-layout exactness.**  K2-K5 used opaque
   `_kevlar_inner` shims at offsets we chose.  Real
   simplefb.ko is compiled against Linux's `<linux/wait.h>`,
   `<linux/device.h>`, etc. — the offsets are *Linux's*,
   not ours.  K6 reconciles every K2-K5 struct: same byte
   size, same field offsets, same field types as Linux
   7.0's UAPI headers.
2. **Real page-table-mapping `ioremap`.**  simplefb's
   framebuffer lives at a fixed PA outside the direct map.
   K6 allocates VA from the vmalloc area, maps the PA range
   through the kernel page tables with the right attributes
   (Normal-NC for framebuffers).
3. **`struct fb_info` + `register_framebuffer`.**  The
   framebuffer-class registration path.  Includes
   `fb_info`'s ~30 fields modules read directly.
4. **Device-tree node parsing.**  simplefb gets its
   framebuffer base/size/format from a DT node with
   `compatible = "simple-framebuffer"`.  We need to expose
   either a real DT node (QEMU virt synthesizes one) or a
   fake one that points at a buffer we provide.

K6 is a much bigger milestone than K1-K5 — closer to a
month of work than a session.  The payoff is enormous:
after K6, the kABI compatibility claim becomes literal.
Linux modules build with their normal toolchain against
their normal headers, drop into Kevlar's initramfs, and
run.

That's "drop-in Linux kernel for binary modules" stopping
being aspiration and becoming fact.
