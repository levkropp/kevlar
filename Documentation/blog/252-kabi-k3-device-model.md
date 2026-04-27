# 252 — kABI K3: the Linux device-model spine

K3 lands.  Loaded `.ko` modules can now register a
`platform_device` and a matching `platform_driver` from the
same module; the kernel-side bus walks the registry, runs the
name-compare match, and fires the driver's `probe()` callback.

The proof on serial, end-to-end across all three milestones in
one boot:

```
kabi: loading /lib/modules/hello.ko          (K1)
kabi: my_init returned 0
kabi: runtime initialized (workqueue + platform bus)
kabi: loading /lib/modules/k2.ko             (K2)
[mod] [k2] work handler running on worker thread
kabi: k2 init_module returned 0
kabi: loading /lib/modules/k3.ko             (K3)
kabi: /lib/modules/k3.ko license=Some("GPL") author=Some("Kevlar")
       desc=Some("kABI K3 demo: platform_device + platform_driver bind")
kabi: applied 33 relocations (3 trampoline(s))
[mod] [k3] init begin
[mod] [k3] platform_device_register ok
[mod] [k3] probe called          ← fired during driver_register's match-walk
[mod] [k3] platform_driver_register ok
[mod] [k3] probe_called observed
[mod] [k3] init done
kabi: k3 init_module returned 0
```

`make ARCH=arm64 test-module-k3` is the regression target.  K1
and K2's tests continue to pass.

## Why device model is the structural milestone

K1 was plumbing.  K2 was utilities.  K3 is the first milestone
where the kernel and the loaded module *register* something
on a shared registry and *that registry drives behavior*.

Real Linux drivers don't just call `printk` and return.  They
declare themselves as a driver (with a `probe` callback, a
`remove` callback, a name) and register on a *bus*.  The bus
maintains two lists — registered drivers, registered devices —
and runs a `match()` callback over every (device, driver) pair.
On match, the bus calls the driver's `probe(device)`.

PCI drivers register on `pci_bus_type`.  Platform drivers
register on `platform_bus_type`.  USB, SCSI, virtio, MMC, SDIO,
all of them — same shape, different bus.  K3 implements the
shape.  Once the spine exists, every later milestone (file
operations, fb_info, drm_device) hangs off it.

## The pieces

K3 adds five new files under `kernel/kabi/`:

```
kref.rs         — atomic refcount handle (kref_init/get/put/read)
kobject.rs      — minimal kobject (no sysfs in K3)
device.rs       — struct device + initialize/add/register/get_*/dev_*_drvdata
bus.rs          — struct device_driver + struct bus_type +
                  driver_register + bus_register + match-walk core
platform.rs     — platform_bus_type singleton + platform_device_register +
                  platform_driver_register + container_of thunks
```

Plus `ksym_static!` (a sibling of K1's `ksym!`) for exporting
kernel-side static data — `platform_bus_type` is a singleton
struct that modules `extern` and reference by address.

Total: ~1100 lines, mostly registration bookkeeping and offset
arithmetic.

## The match algorithm

The whole device-model spine reduces to:

```rust
fn add_driver(bus, drv) {
    bus.drivers.push(drv);
    for dev in bus.devices {
        if bus.match(dev, drv) != 0 {
            dev.driver = drv;
            drv.probe(dev);
        }
    }
}

fn add_device(bus, dev) {
    bus.devices.push(dev);
    for drv in bus.drivers {
        if bus.match(dev, drv) != 0 {
            dev.driver = drv;
            drv.probe(dev);
            break;  // Linux: one driver per device.
        }
    }
}
```

That's the heart of every Linux bus probe path.

The `match` callback is per-bus.  Real Linux's
`platform_match` chains through five fallbacks:

1. `driver_override` — exact match against an override name
2. `of_match` — device-tree compatible string
3. `acpi_match` — ACPI device IDs
4. `id_table` — table-of-IDs comparison
5. `name strcmp` — last-resort name compare

K3 implements only #5.  The others stub to "no match."  When
real driver modules surface that need `of_match`, K6+
extends the platform_match chain.

## container_of for the probe thunk

The trickiest piece in K3 is the indirection that lets the
generic `bus.match(dev, drv)` and `drv.probe(dev)` paths fire
the *platform-specific* probe (which expects a
`struct platform_device *`, not a `struct device *`).

Linux solves this with `container_of`: given a pointer to an
embedded field and the offset of that field in the wrapper,
recover the wrapper.

```rust
fn pdev_of_dev(dev: *mut DeviceShim) -> *mut PlatformDeviceShim {
    let off = core::mem::offset_of!(PlatformDeviceShim, dev);
    unsafe { (dev as *mut u8).sub(off) as *mut PlatformDeviceShim }
}
```

`platform_driver_register` substitutes its own thunks for the
generic `device_driver.probe`/`remove` slots:

```rust
unsafe {
    let drv = &raw mut (*pdrv).driver;
    (*drv).probe = Some(platform_drv_probe);   // thunk
    (*drv).remove = Some(platform_drv_remove); // thunk
    add_driver(&raw const platform_bus_type, drv);
}
```

When the bus calls `drv.probe(dev)`, our thunk recovers both
the `platform_device *` and the user's `platform_driver *`,
then forwards:

```rust
extern "C" fn platform_drv_probe(dev: *mut DeviceShim) -> i32 {
    let pdev = pdev_of_dev(dev);
    let pdrv = pdrv_of_drv(unsafe { (*dev).driver });
    if let Some(probe) = unsafe { (*pdrv).probe } {
        return probe(pdev);
    }
    0
}
```

This is exactly the indirection real Linux uses
(`platform_drv_probe` in `drivers/base/platform.c`).  Our
implementation is one extern "C" function long.

## Layout: still opaque

K3 keeps K2's strategy of opaque `_kevlar_inner` slots.  Our
`struct device` looks like:

```c
struct device {
    void                       *_kevlar_inner;  // refcount + bound flag
    struct device              *parent;
    const struct bus_type      *bus;
    struct device_driver       *driver;
    void                       *driver_data;
    const char                 *init_name;
};
```

Real Linux's `struct device` is hundreds of bytes — `kobject
kobj` at offset 0, then mutex, parent, links, power, dma_ops,
dma_mask, of_node, fwnode, devres_lock, class, groups, release,
iommu — and bitfields, conditional fields for NUMA / IOMMU.
K3's six fields are the bare minimum the demo and any
near-term real driver actually *reads*.

The rule is: any field a module reads or writes from C lives at
a known offset in our header.  Anything else is in
`_kevlar_inner`, an opaque pointer to a heap struct that we
control.  This is the same shape K2 used for `wait_queue_head`,
`completion`, and `work_struct`.

K3 is still *not* binary-compatible with prebuilt Linux
modules.  That's K6.  By design — adding hundreds of phantom
fields right now would be effort spent on layout matching
without any binary actually testing the layouts.  K6 reconciles
when the first prebuilt `simplefb.ko` lands and forces the
issue.

## Demo

`testing/k3-module.c`:

```c
#include "kevlar_kabi_k3.h"

MODULE_LICENSE("GPL");
MODULE_AUTHOR("Kevlar");
MODULE_DESCRIPTION("kABI K3 demo: platform_device + platform_driver bind");

static int probe_called;

static int k3_probe(struct platform_device *pdev) {
    probe_called = 1;
    printk("[k3] probe called\n");
    return 0;
}

static struct platform_device k3_pdev = { .name = "k3-demo", .id = 0 };
static struct platform_driver k3_pdrv = {
    .probe  = k3_probe,
    .driver = { .name = "k3-demo" },
};

int init_module(void) {
    printk("[k3] init begin\n");
    platform_device_register(&k3_pdev);
    printk("[k3] platform_device_register ok\n");
    platform_driver_register(&k3_pdrv);
    printk("[k3] platform_driver_register ok\n");
    if (probe_called) {
        printk("[k3] probe_called observed\n");
    }
    printk("[k3] init done\n");
    return 0;
}
```

Notice the order: `platform_device_register` happens first.
At that moment no driver matches, so the device sits unbound.
Then `platform_driver_register` walks the device list, hits the
name-compare match, and calls `probe()` *inside the
`platform_driver_register` call*.  By the time we read
`probe_called`, it's already 1.

That's the bus's match-walk firing in real time, before the
registration call returns.  Identical semantics to Linux.

## What surfaced during the build

One real diagnostic and one good-to-find: the `ksym!()` macro
from K1 cast `$func as *const ()` — fine for function
pointers, broken for static structs.  The Rust 2024 cast rules
disallow `BusTypeShim as *const ()`.  Added a sibling
`ksym_static!()` macro that does `&raw const $item as *const ()`,
used it for `platform_bus_type`.  Two-line fix.

That's the only bug-class issue K3 hit — the rest of the
implementation worked on first build.  The k3.ko module's 33
relocations all fell into types K1's relocator already handles
(ADR_PREL_LO21, CALL26).  3 trampolines (one each for
`printk`, `platform_device_register`, `platform_driver_register`
— all kernel-side targets >128MB from the heap-loaded module).

## Cumulative kABI surface (K1 + K2 + K3)

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
```

~50 symbols.  Linear scan is still sub-microsecond at this
size; binary search defers to whenever the count crosses ~200.

## What K3 didn't do

- **sysfs.**  No `/sys/devices/...`.  `kobject_add` is a no-op.
  When a module needs to expose attributes via
  `device_create_file`, K6+ adds it.
- **`struct class`.**  Class-based device organization
  (`class_create("input")`).  Not needed for platform.
- **PCI bus.**  Real `pci_bus_type` + PCI config-space
  scanning is K4-K5-ish, layered atop the device-model spine.
- **Module unload.**  Demo modules are init-only; K3 doesn't
  test `driver_unregister` cleanup paths even though the code
  is written.
- **DT / ACPI matching.**  Only name-compare matches.
- **PM ops.**  No suspend/resume/shutdown plumbing.
- **Linux struct-layout exactness.**  K6.

## Status

| Surface | Status |
|---|---|
| K1 — ELF .ko loader | ✅ |
| K2 — kmalloc / wait / work / completion | ✅ |
| K3 — device model + platform bind/probe | ✅ |
| K4 — file_operations + char-device bridge | ⏳ next |
| K5-K9 | ⏳ |

## What K4 looks like

K4 is the first milestone where the *userspace* sees the
kernel module's existence.  A char-device driver registers
under a major/minor number; userspace `open("/dev/foo")`s the
node; reads land in the module's `read` callback.  The pieces:

- `register_chrdev_region` / `alloc_chrdev_region` /
  `cdev_init` / `cdev_add`
- `struct file_operations` with `.open`, `.read`, `.write`,
  `.release`
- `class_create` / `device_create` to make a `/dev/` node
  appear (or, for K4, just a manual `mknod`-equivalent — a
  static `/dev/foo` entry registered into Kevlar's existing
  devfs)

The K4 demo: a module exposes `/dev/k4-demo`, userspace `cat`s
it, the module's `read` callback returns "hello from k4\n".

This is where Kevlar starts to *feel* like a Linux kernel from
the userspace side: a loaded `.ko` binary affects what
`/dev/...` paths exist and what reads from them return.  After
K4, K5 adds the I/O primitives (vmalloc-non-contiguous, MMIO,
DMA) that any real device driver needs to actually talk to
hardware.
