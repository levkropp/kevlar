# M5 Phase 5: VirtIO Block Driver

Kevlar can now read and write disk sectors. The VirtIO block driver gives
the kernel its first access to persistent storage — the hardware foundation
for ext2 filesystem support in Phase 6.

## VirtIO Block Protocol

VirtIO is a standardized interface for virtual I/O devices. We already have
a VirtIO-net driver for networking, so the core transport infrastructure
(PCI device discovery, virtqueue setup, interrupt handling) already exists.
The block driver adds a new device type on top of this.

Each block request is a chain of three descriptors on a single virtqueue:

```
┌─────────────────┐     ┌──────────────┐     ┌────────────┐
│ BlockReqHeader   │ --> │ Data buffer  │ --> │ Status byte│
│ (type, sector)   │     │ (512*n bytes)│     │ (1 byte)   │
│ device-readable  │     │ dev-r or w   │     │ dev-writable│
└─────────────────┘     └──────────────┘     └────────────┘
```

The header tells the device what to do (read or write) and which sector.
The data buffer carries the payload. The status byte tells us if it worked.

## Implementation

### Device Discovery

The driver registers as a `DeviceProber` alongside virtio-net. PCI probing
checks for vendor 0x1AF4 with device ID 0x1042 (modern) or 0x1001
(transitional). MMIO probing checks for device type 2. Both paths fall
through to the same `VirtioBlk::new()` initialization.

### Request Buffer Layout

A pre-allocated 2-page buffer holds all request metadata:

- `[0..16)`: request header (type, reserved, sector)
- `[16..17)`: status byte (device writes completion status here)
- `[PAGE_SIZE..2*PAGE_SIZE)`: data buffer (up to 8 sectors at once)

This avoids per-request allocation. The three descriptor chain entries
point to offsets within this buffer.

### Synchronous Completion

The initial implementation uses spin-wait completion: enqueue the descriptor
chain, notify the device, then poll the used ring until the device returns
the completed chain. This is simple and correct. Interrupt-driven async
completion can be added later when filesystem workloads demand it.

### Block Cache

A 256-entry direct-mapped cache (128 KiB) sits between callers and the
device. Cache lookups are `O(1)` via `sector % 256`. Reads populate the
cache on miss. Writes use write-through semantics — the sector is written
directly to the device and the cache entry is invalidated.

The cache is critical for ext2 performance: the superblock, group
descriptors, and inode tables are read repeatedly during filesystem
operations. Without caching, each metadata access would be a full
device roundtrip.

### BlockDevice Trait

The driver exposes a `BlockDevice` trait in `kevlar_api::driver::block`:

```rust
pub trait BlockDevice: Send + Sync {
    fn read_sectors(&self, start_sector: u64, buf: &mut [u8]) -> Result<(), BlockError>;
    fn write_sectors(&self, start_sector: u64, buf: &[u8]) -> Result<(), BlockError>;
    fn flush(&self) -> Result<(), BlockError>;
    fn capacity_bytes(&self) -> u64;
    fn sector_size(&self) -> u32;
}
```

A global registry holds one block device. The ext2 filesystem (Phase 6)
will use `block_device()` to obtain it without knowing anything about
VirtIO.

## Self-Test

The driver runs a self-test during initialization:

1. Read the first 4 sectors — checks for ext2 magic number (0xEF53)
2. Write a pattern to the last sector, read it back, verify match
3. Restore the original sector content

```
virtio-blk: capacity = 131072 sectors (64 MiB)
virtio-blk: read OK (ext2 superblock detected)
virtio-blk: write-readback OK
virtio-blk: driver initialized
```

## QEMU Integration

`make disk` creates a 64 MiB ext2 disk image. `make run-disk` boots with
it attached. The `run-qemu.py` script gained a `--disk` flag that passes
the image to QEMU as a VirtIO block device — using `if=virtio` for x86_64
PCI and `virtio-blk-device` for ARM64 MMIO.

## What's Next

Phase 6 implements the ext2 filesystem on top of this block device,
giving Kevlar the ability to mount real disk partitions and access files
on persistent storage.
