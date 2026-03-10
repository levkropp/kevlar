# Phase 5: VirtIO Block Driver

**Goal:** Implement a VirtIO block device driver, giving Kevlar the ability to
read and write disk sectors. This is the hardware foundation for real
filesystems.

## Background

VirtIO is a standardized interface for virtual I/O devices. We already have a
VirtIO-net driver (network), so the core VirtIO transport infrastructure
(PCI device discovery, virtqueue setup, interrupt handling) already exists.
The block driver adds a new device type on top of this infrastructure.

## Design

### Device Discovery

VirtIO block devices are identified by:
- **PCI:** Vendor 0x1AF4, Device 0x1001 (transitional) or 0x1042 (modern)
- **MMIO (ARM64):** Compatible string "virtio,mmio" with device type 2

Discovery hooks into the existing VirtIO PCI/MMIO probe path. When a block
device is found, create a `VirtioBlockDevice` and register it with a global
block device registry.

### VirtIO Block Protocol

The device uses a single virtqueue for requests. Each request is a chain of
3 descriptors:

```
┌─────────────────┐     ┌──────────────┐     ┌────────────┐
│ BlockReqHeader   │ --> │ Data buffer  │ --> │ Status byte│
│ (type, sector)   │     │ (512*n bytes)│     │ (1 byte)   │
│ device-readable  │     │ dev-r or w   │     │ dev-writable│
└─────────────────┘     └──────────────┘     └────────────┘
```

Request types:
- `VIRTIO_BLK_T_IN` (0) — read sectors
- `VIRTIO_BLK_T_OUT` (1) — write sectors
- `VIRTIO_BLK_T_FLUSH` (4) — flush cache
- `VIRTIO_BLK_T_GET_ID` (8) — get device ID string

Status codes:
- `VIRTIO_BLK_S_OK` (0) — success
- `VIRTIO_BLK_S_IOERR` (1) — I/O error
- `VIRTIO_BLK_S_UNSUPP` (2) — unsupported request

### Block Device Interface

```rust
pub struct VirtioBlockDevice {
    virtqueue: VirtQueue,
    capacity_sectors: u64,   // from device config
    sector_size: u32,        // usually 512
    // Request completion tracking.
    pending: SpinLock<VecDeque<BlockRequest>>,
}

pub trait BlockDevice: Send + Sync {
    fn read_sectors(&self, start_sector: u64, buf: &mut [u8]) -> Result<()>;
    fn write_sectors(&self, start_sector: u64, buf: &[u8]) -> Result<()>;
    fn flush(&self) -> Result<()>;
    fn capacity_bytes(&self) -> u64;
    fn sector_size(&self) -> u32;
}
```

### Request Flow

1. Caller invokes `read_sectors(sector, buf)`
2. Allocate descriptors: header + data buffer + status byte
3. Add descriptor chain to virtqueue, notify device (write to queue notify reg)
4. Wait for completion:
   - **Synchronous (initial):** Spin-wait on status byte change
   - **Interrupt-driven (later):** Sleep on wait queue, wake from IRQ handler
5. Check status byte, return result

### Block Cache

Raw sector reads are expensive (even virtualized). Add a simple block cache:

```rust
const CACHE_SIZE: usize = 256;  // 256 * 512 = 128 KiB

struct BlockCache {
    entries: [CacheEntry; CACHE_SIZE],
}

struct CacheEntry {
    sector: u64,
    dirty: bool,
    data: [u8; 512],
    valid: bool,
}
```

Start with direct-mapped (sector % CACHE_SIZE). Upgrade to LRU if needed
based on profiling. The cache is critical for ext2 metadata reads — the
superblock, group descriptors, and inode tables are read repeatedly.

### Interrupt Handling

VirtIO block devices signal completion via MSI-X or legacy interrupts (same
mechanism as virtio-net). The IRQ handler:

1. Read ISR status register to acknowledge
2. Process completed descriptors from used ring
3. Wake any sleeping threads waiting for completion

### QEMU Integration

QEMU command line for VirtIO block:

```bash
qemu-system-x86_64 ... \
    -drive file=disk.img,format=raw,if=virtio,readonly=on

# ARM64:
qemu-system-aarch64 ... \
    -drive file=disk.img,format=raw,if=virtio,readonly=on
```

Create test disk images with:

```bash
# Create a 64 MiB ext2 image
dd if=/dev/zero of=disk.img bs=1M count=64
mkfs.ext2 disk.img
# Mount and populate
sudo mount -o loop disk.img /mnt
sudo cp test_binary /mnt/
sudo umount /mnt
```

## Placement in Ringkernel Architecture

The VirtIO block driver is **Platform/Ring 0** code because it interacts
directly with hardware (MMIO registers, DMA buffers, interrupts). It should
live in the `platform` crate alongside the existing VirtIO-net driver.

The `BlockDevice` trait is the boundary between Platform and Core/Services.
The ext2 filesystem (Phase 6) uses `BlockDevice` without knowing it's VirtIO.

## Reference Sources

- VirtIO specification v1.2 (OASIS standard) — Section 5.2 Block Device
- Existing Kevlar VirtIO-net driver — queue/transport infrastructure
- FreeBSD `sys/dev/virtio/block/virtio_blk.c` (BSD-2-Clause)

## Testing

- Device detected during PCI/MMIO probe, capacity reported
- Read sector 0 (MBR/superblock) returns non-zero data
- Read multiple consecutive sectors returns consistent data
- Write + read-back returns written data
- Block cache hit rate > 80% for sequential metadata reads
- ARM64: same tests via VirtIO-MMIO transport
