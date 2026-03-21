# M10.5 Phase 3: Storage Drivers

**Goal:** Load `nvme.ko` and/or `ahci.ko` from Linux 6.18, expose storage
devices as block devices, and boot Alpine from a real NVMe SSD or SATA HDD.

---

## Target drivers

| Driver | Devices | .ko files |
|--------|---------|-----------|
| NVMe | PCIe SSDs (Samsung, WD, SK Hynix, ...) | `nvme.ko` + `nvme-core.ko` |
| AHCI | SATA HDDs/SSDs via AHCI controller | `libahci.ko` + `ahci.ko` |
| virtio-blk | QEMU virtual disk (already native) | N/A |

Start with NVMe — it's simpler (no legacy DMA modes, pure PCIe/MSI-X) and
covers most modern hardware. AHCI covers older machines and SATA-attached drives.

---

## Block layer shim

Drivers don't write to disk directly — they register a block device and
the kernel's block layer routes I/O through them. The block layer shim is
the new infrastructure this phase introduces.

### Key structs

```c
struct gendisk {
    int major, first_minor, minors;
    char disk_name[DISK_NAME_LEN];
    struct request_queue *queue;
    const struct block_device_operations *fops;
    void *private_data;
    // ... sector count, partitions, etc.
};

struct request_queue {
    struct blk_mq_tag_set *tag_set;
    // ... queue limits, scheduler, etc.
};
```

### Key functions

| Function | Implementation |
|----------|----------------|
| `alloc_disk(minors)` / `blk_alloc_disk(...)` | Allocate `struct gendisk` |
| `add_disk(disk)` | Register block device; make it accessible via `/dev/` |
| `del_gendisk(disk)` | Unregister |
| `put_disk(disk)` | Release reference |
| `set_capacity(disk, sectors)` | Set disk size in 512-byte sectors |
| `blk_mq_alloc_tag_set(set)` | Allocate multi-queue tag set |
| `blk_mq_init_queue(set)` | Initialize request queue |
| `blk_mq_free_tag_set(set)` | Free tag set |
| `blk_mq_start_request(rq)` | Mark request as in-flight |
| `blk_mq_end_request(rq, err)` | Complete request, signal caller |
| `blk_rq_map_sg(q, rq, sglist)` | Map request to scatter-gather list |
| `blk_queue_max_hw_sectors(q, sectors)` | Set I/O size limits |
| `blk_queue_logical_block_size(q, size)` | Set block size |

### I/O flow

```
Kevlar VFS read() / write()
      │
      ▼
block layer shim  ──────────────────────────────────────────────────────┐
  alloc struct request                                                   │
  call driver's .queue_rq() with the request                            │
  sleep (wait for completion)                                            │
      │                                                                  │
      ▼                                                                  │
NVMe/AHCI driver                                                         │
  fill hardware submission queue entry                                   │
  ring doorbell register                                                 │
  ← hardware DMA completes, MSI-X fires →                               │
  call blk_mq_end_request()  ─────────────────────────────────────────────┘
      │
      ▼
block layer: wake sleeping caller, return data
```

### Partition support

The block layer must scan the disk's partition table (GPT or MBR) after
`add_disk()` and create sub-devices:
- `/dev/sda` → whole disk (major 8, minor 0)
- `/dev/sda1` → first partition (major 8, minor 1)
- `/dev/nvme0n1` → NVMe namespace (major 259, minor 0)
- `/dev/nvme0n1p1` → first NVMe partition (major 259, minor 1)

Start with MBR (simpler). GPT is needed for EFI systems.

---

## NVMe specifics

NVMe drivers use `nvme-core.ko` as a library:

```
nvme.ko (PCIe transport)
  └── depends on nvme-core.ko (core NVMe logic: admin queues, namespaces, I/O)
```

kcompat must support inter-module dependencies: when `nvme.ko` is loaded,
automatically load `nvme-core.ko` first, then resolve cross-module symbols.

### NVMe initialization sequence

1. `pci_register_driver` → kcompat finds NVMe PCI device (class 0x010802)
2. `probe()`: `pci_enable_device` → `pci_iomap` (BAR 0) → map NVMe registers
3. Enable MSI-X, request IRQs for each queue
4. Submit `Identify Controller` admin command via admin queue
5. Submit `Identify Namespace` for each namespace
6. Call `nvme_alloc_ns()` → `add_disk()` → block device appears

---

## AHCI specifics

AHCI is more complex (legacy DMA, port multipliers, FIS-based switching):

```
ahci.ko (AHCI port driver)
  └── depends on libahci.ko (AHCI core logic)
       └── depends on libata.ko (ATA core: command encoding, error handling)
```

Three-module chain. libata is a significant subsystem (~20K lines). Options:
1. Implement minimal `libata` kcompat (just enough for AHCI)
2. Use an alternative SATA driver that has fewer dependencies
3. Defer AHCI to after NVMe works

Recommended: implement NVMe first (simpler dependency chain), add AHCI in
a follow-up. Most modern hardware has NVMe.

---

## Device nodes

After `add_disk()`, the block device needs a `/dev/` node. kcompat notifies
Kevlar's device system:

```rust
// In kcompat block layer, after add_disk():
kevlar_register_block_device(disk_name, major, minor, &ops);
// Creates /dev/nvme0n1 with the correct major:minor
```

Kevlar's existing device dispatch (mknod infrastructure from M10 Phase 5)
routes open() on the device node to the kcompat block layer, which forwards
to the driver.

---

## Filesystem integration

Once the block device is registered, Kevlar's existing `mount()` syscall
can mount ext4 or ext2 from it:

```
mount("/dev/nvme0n1p2", "/", "ext4", MS_RDONLY, "")
```

This reuses Kevlar's existing filesystem code — no new filesystem work needed.
The block device appears as a `FileLike` backed by the kcompat driver.

---

## Verification

### QEMU test (before real hardware)

```bash
# Create a disk image with a filesystem
dd if=/dev/zero of=/tmp/disk.img bs=1M count=512
mkfs.ext4 /tmp/disk.img

# Boot with NVMe emulation
qemu-system-x86_64 ... \
  -drive file=/tmp/disk.img,if=none,id=d0 \
  -device nvme,drive=d0,serial=deadbeef
```

Expected: Kevlar loads `nvme.ko`, `/dev/nvme0n1` appears, `mount` succeeds.

### Real hardware

Boot Kevlar on a machine with an NVMe drive. Expected:
- `insmod nvme-core.ko nvme.ko` succeeds
- `/dev/nvme0n1` appears with correct sector count
- `mount /dev/nvme0n1p1 /mnt -o ro` mounts the existing partition
- Files readable from the mounted filesystem

---

## Files to create/modify

- `kernel/kcompat/block.rs` — block layer shim (`gendisk`, `request_queue`, `blk_mq_*`)
- `kernel/kcompat/partition.rs` — MBR/GPT partition table parser
- `kernel/kcompat/symbols_6_18.rs` — add NVMe/AHCI symbols
- `kernel/device/block.rs` — register kcompat block devices with Kevlar
