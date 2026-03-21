# M10.5 Phase 2: Core Subsystems (kcompat shim)

**Goal:** Implement the core Linux kernel subsystems that Tier 1 drivers
depend on. Success criterion: a PCI driver module can enumerate PCI devices
without crashing.

---

## Overview

Tier 1 drivers (NVMe, AHCI, e1000e, r8169) share a common dependency set.
This phase implements that set as the kcompat shim layer. The shim's job is:

1. Present the correct Linux 6.18 struct layouts (field offsets must match)
2. Implement the functions drivers call (may be thin wrappers or no-ops)
3. Map Linux primitives to Kevlar equivalents where they exist

Not every function needs a full implementation. Many can be stubs that
return success — drivers generally handle errors gracefully.

---

## PCI subsystem

### Structs

`struct pci_dev` is the central struct. Drivers receive a `*mut pci_dev`
and access fields directly:

```c
// Fields drivers commonly access:
struct pci_dev {
    struct list_head bus_list;  // 0x00 — must be correct
    struct pci_bus  *bus;       // 0x10
    unsigned int    devfn;      // 0x18 — BDF address
    unsigned short  vendor;     // 0x1c
    unsigned short  device;     // 0x1e
    unsigned short  subsystem_vendor; // 0x20
    unsigned short  subsystem_device; // 0x22
    unsigned int    class;      // 0x24
    u8              revision;   // 0x28
    struct pci_driver *driver;  // 0x98
    u64             dma_mask;   // 0xa0
    struct device   dev;        // 0xb0 — embedded struct device
    int             irq;        // 0x1e0 (approx)
    struct resource resource[DEVICE_COUNT_RESOURCE]; // BARs
    // ... many more fields
};
```

We copy the exact 6.18 definition (field by field, with matching offsets)
and add `static_assert!` checks to verify. Fields Kevlar doesn't use are
zeroed in the allocated struct.

### Functions

| Function | Implementation |
|----------|----------------|
| `pci_enable_device(dev)` | Mark device enabled; configure command register |
| `pci_disable_device(dev)` | Mark disabled |
| `pci_request_regions(dev, name)` | Reserve BAR regions (stub → 0) |
| `pci_release_regions(dev)` | Release BARs (stub → no-op) |
| `pci_iomap(dev, bar, max)` | Map BAR into kernel VA → `ioremap(BAR_paddr, size)` |
| `pci_iounmap(dev, addr)` | Unmap BAR → `iounmap(addr)` |
| `pci_read_config_byte/word/dword` | Read from PCI config space |
| `pci_write_config_byte/word/dword` | Write to PCI config space |
| `pci_set_master(dev)` | Enable bus mastering in command register |
| `pci_enable_msi(dev)` | Enable MSI interrupt |
| `pci_enable_msix_range(dev, ...)` | Enable MSI-X interrupts |
| `pci_disable_msi(dev)` | Disable MSI |
| `pci_find_capability(dev, cap)` | Scan PCI capability list |
| `pci_register_driver(drv)` | Register driver; probe matching devices |
| `pci_unregister_driver(drv)` | Unregister driver |

`pci_register_driver` is the entry point. When a module calls it, kcompat
enumerates Kevlar's PCI device list, checks vendor/device IDs against the
driver's `id_table`, and calls `drv->probe(dev, id)` for each match.

Kevlar already enumerates PCI devices for virtio. The kcompat layer exposes
this list to loaded modules.

---

## IRQ framework

### Functions

| Function | Implementation |
|----------|----------------|
| `request_irq(irq, handler, flags, name, dev)` | Register handler for IRQ number; map to Kevlar's IRQ registration |
| `free_irq(irq, dev)` | Unregister handler |
| `request_threaded_irq(irq, top, bottom, ...)` | Threaded IRQ → schedule Kevlar task on interrupt |
| `disable_irq(irq)` | Mask IRQ at LAPIC/IOAPIC |
| `enable_irq(irq)` | Unmask IRQ |
| `irq_set_affinity_hint(irq, mask)` | Stub → 0 (affinity hints are optional) |

Linux IRQ numbers in the kcompat shim map directly to Kevlar's IRQ numbers.
MSI/MSI-X IRQ numbers are allocated by the PCI subsystem when MSI is enabled.

### `irqreturn_t`

Handlers return `IRQ_HANDLED` (1) or `IRQ_NONE` (0). Threaded handlers also
return `IRQ_WAKE_THREAD`. These are simple `c_int` values.

---

## DMA API

DMA is where drivers interact most directly with hardware memory. kcompat
must correctly manage physical addresses.

| Function | Implementation |
|----------|----------------|
| `dma_alloc_coherent(dev, size, dma_handle, gfp)` | Alloc pages, return VA + PA in `*dma_handle` |
| `dma_free_coherent(dev, size, vaddr, dma_handle)` | Free pages |
| `dma_map_single(dev, ptr, size, dir)` | Return physical address of kernel VA |
| `dma_unmap_single(dev, dma_handle, size, dir)` | No-op (no IOMMU in Tier 1) |
| `dma_map_sg(dev, sglist, nents, dir)` | Map scatter-gather list; fill `dma_address` fields |
| `dma_unmap_sg(dev, sglist, nents, dir)` | No-op |
| `dma_pool_create/alloc/free/destroy` | Small DMA buffer pool |
| `dma_set_mask(dev, mask)` | Validate DMA address mask; stub → 0 |
| `dma_set_coherent_mask(dev, mask)` | Stub → 0 |

In Kevlar without an IOMMU, physical == DMA address (identity mapping).
`dma_alloc_coherent` is just `alloc_pages` + return the paddr as `dma_handle`.

---

## Work queues

Work queues are used for deferred work (bottom halves, async tasks).

| Function | Implementation |
|----------|----------------|
| `INIT_WORK(work, func)` | Initialize `struct work_struct` |
| `schedule_work(work)` | Enqueue on global work queue → spawn Kevlar task |
| `schedule_delayed_work(work, delay)` | Same with timer |
| `cancel_work_sync(work)` | Wait for in-flight work to complete |
| `alloc_workqueue(name, flags, max_active)` | Create named work queue |
| `destroy_workqueue(wq)` | Destroy, wait for pending work |
| `queue_work(wq, work)` | Submit to specific work queue |

Work queues map to Kevlar's existing thread/task infrastructure. Each work
queue becomes a pool of kernel threads executing closures.

---

## Device model

The Linux device model (`struct device`, `struct bus_type`) is complex but
most drivers only use a subset:

| Function | Implementation |
|----------|----------------|
| `dev_name(dev)` | Return device name string |
| `dev_err/warn/info/dbg(dev, fmt, ...)` | `printk`-based logging with device prefix |
| `get_device(dev)` / `put_device(dev)` | Reference counting (stub or real) |
| `device_initialize(dev)` | Zero-init + set kobj refcount |
| `device_add(dev)` | Register device; add sysfs entries (stub) |
| `device_del(dev)` | Unregister |

Sysfs integration can be stubbed — `device_add` succeeds but creates no
real sysfs entries. Drivers don't fail if sysfs is absent.

---

## Memory allocation

Linux drivers use `kmalloc`/`kfree` everywhere:

| Function | Kevlar equivalent |
|----------|-------------------|
| `kmalloc(size, gfp)` | `alloc::alloc::alloc(layout)` |
| `kzalloc(size, gfp)` | `alloc::alloc::alloc_zeroed(layout)` |
| `kfree(ptr)` | `alloc::alloc::dealloc(ptr, layout)` — need to track layout |
| `krealloc(ptr, new_size, gfp)` | `alloc::alloc::realloc(...)` |
| `vmalloc(size)` | Large virtually-contiguous allocation |
| `vfree(ptr)` | Free vmalloc region |
| `kmemdup(src, len, gfp)` | Alloc + memcpy |
| `kstrdup(str, gfp)` | Alloc + strcpy |

The challenge with `kfree` is that it takes only a pointer, not a size.
Options:
1. Use Kevlar's allocator which tracks sizes internally (preferred)
2. Maintain a side table of `ptr → layout` (expensive)

---

## Atomics and locking

Linux's `spinlock_t`, `mutex`, `rwlock_t`, `atomic_t` must have correct
sizes (drivers may embed them in their own structs).

| Linux type | kcompat implementation |
|-----------|----------------------|
| `spinlock_t` | `u32` (same size as Linux's; ops use Kevlar SpinLock |
| `mutex` | `u64` (contains Linux's `struct mutex` equivalent) |
| `atomic_t` | `AtomicI32` (4 bytes, matches Linux) |
| `atomic64_t` | `AtomicI64` (8 bytes, matches Linux) |
| `refcount_t` | `AtomicU32` with saturation |
| `rwlock_t` | `u32` (read-write spinlock) |
| `completion` | Future/wait event primitive |

---

## Timer API

| Function | Implementation |
|----------|----------------|
| `timer_setup(timer, func, flags)` | Initialize `struct timer_list` |
| `mod_timer(timer, expires)` | Arm timer (jiffies-based) |
| `del_timer_sync(timer)` | Cancel, wait for in-flight callback |
| `jiffies` | Global tick counter (Hz=100 in Kevlar) |
| `HZ` | Constant 100 |
| `msecs_to_jiffies(ms)` | `ms / 10` |
| `jiffies_to_msecs(j)` | `j * 10` |

Timer callbacks fire in a dedicated kernel thread context, not interrupt context.

---

## I/O memory access

| Linux macro | Implementation |
|-------------|----------------|
| `ioremap(paddr, size)` | Map MMIO region into kernel VA |
| `iounmap(vaddr)` | Unmap |
| `readb/readw/readl/readq(addr)` | Volatile read from MMIO |
| `writeb/writew/writel/writeq(val, addr)` | Volatile write |
| `ioread32/iowrite32(...)` | Same as read/writel |
| `memcpy_fromio` / `memcpy_toio` | MMIO bulk copy |

Kevlar's `platform/x64/mmio.rs` already handles MMIO mapping. kcompat wraps it.

---

## Verification

Load a PCI probe-only module:

```c
// pci_probe_test.ko — probes for any Intel device, prints its config
static int probe(struct pci_dev *dev, const struct pci_device_id *id) {
    u32 val;
    pci_read_config_dword(dev, 0, &val);
    dev_info(&dev->dev, "Found PCI device %04x:%04x, config[0]=%08x\n",
             dev->vendor, dev->device, val);
    return -ENODEV; // don't actually claim it
}

static struct pci_device_id ids[] = {
    { PCI_VENDOR_ID_INTEL, PCI_ANY_ID, PCI_ANY_ID, PCI_ANY_ID, 0, 0, 0 },
    { 0 }
};
MODULE_DEVICE_TABLE(pci, ids);

static struct pci_driver test_driver = {
    .name = "pci_probe_test",
    .id_table = ids,
    .probe = probe,
};

module_pci_driver(test_driver);
```

Expected output on `insmod pci_probe_test.ko`:
```
pci_probe_test: Found PCI device 8086:1237, config[0]=12378086
pci_probe_test: Found PCI device 8086:7000, config[0]=70008086
... (one line per PCI device in the QEMU/real system)
```
