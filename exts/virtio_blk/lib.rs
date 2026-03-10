// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! A virtio-blk device driver.
#![no_std]

extern crate alloc;

#[macro_use]
extern crate kevlar_api;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::hint;

use virtio::device::{IsrStatus, Virtio, VirtqDescBuffer};
use virtio::transports::{virtio_mmio::VirtioMmio, VirtioAttachError, VirtioTransport};
#[cfg(target_arch = "x86_64")]
use virtio::transports::{virtio_pci_legacy::VirtioLegacyPci, virtio_pci_modern::VirtioModernPci};

use kevlar_api::address::VAddr;
use kevlar_api::arch::PAGE_SIZE;
use kevlar_api::driver::{
    attach_irq,
    block::{register_block_device, BlockDevice, BlockError},
    register_driver_prober, DeviceProber,
};
#[cfg(target_arch = "x86_64")]
use kevlar_api::driver::pci::PciDevice;
use kevlar_api::driver::VirtioMmioDevice;
use kevlar_api::mm::{alloc_pages, AllocPageFlags};
use kevlar_api::sync::SpinLock;

const SECTOR_SIZE: usize = 512;

// VirtIO block request types.
const VIRTIO_BLK_T_IN: u32 = 0; // Read
const VIRTIO_BLK_T_OUT: u32 = 1; // Write

// VirtIO block status codes.
const VIRTIO_BLK_S_OK: u8 = 0;

// Block request queue index.
const VIRTIO_BLK_QUEUE: u16 = 0;

// Block cache size (number of sectors).
const CACHE_SIZE: usize = 256;

/// VirtIO block request header (16 bytes).
#[repr(C)]
struct VirtioBlkReqHeader {
    type_: u32,
    reserved: u32,
    sector: u64,
}

/// A cached sector.
struct CacheEntry {
    sector: u64,
    valid: bool,
    data: [u8; SECTOR_SIZE],
}

struct BlockCache {
    entries: Vec<CacheEntry>,
}

impl BlockCache {
    fn new() -> Self {
        let mut entries = Vec::with_capacity(CACHE_SIZE);
        for _ in 0..CACHE_SIZE {
            entries.push(CacheEntry {
                sector: 0,
                valid: false,
                data: [0u8; SECTOR_SIZE],
            });
        }
        BlockCache { entries }
    }

    fn index(sector: u64) -> usize {
        (sector as usize) % CACHE_SIZE
    }

    fn get(&self, sector: u64) -> Option<&[u8; SECTOR_SIZE]> {
        let entry = &self.entries[Self::index(sector)];
        if entry.valid && entry.sector == sector {
            Some(&entry.data)
        } else {
            None
        }
    }

    fn put(&mut self, sector: u64, data: &[u8]) {
        let idx = Self::index(sector);
        let entry = &mut self.entries[idx];
        entry.sector = sector;
        entry.valid = true;
        entry.data.copy_from_slice(&data[..SECTOR_SIZE]);
    }

    fn invalidate(&mut self, sector: u64) {
        let idx = Self::index(sector);
        let entry = &mut self.entries[idx];
        if entry.valid && entry.sector == sector {
            entry.valid = false;
        }
    }
}

pub struct VirtioBlk {
    virtio: Virtio,
    capacity_sectors: u64,
    /// Pre-allocated request buffer (2 pages):
    /// [0..16): request header (device-readable)
    /// [16..17): status byte (device-writable)
    /// [PAGE_SIZE..2*PAGE_SIZE): data buffer (up to 8 sectors)
    req_buf: VAddr,
    cache: BlockCache,
}

impl VirtioBlk {
    pub fn new(transport: Arc<dyn VirtioTransport>) -> Result<VirtioBlk, VirtioAttachError> {
        let mut virtio = Virtio::new(transport);
        // No special features needed for basic block I/O.
        virtio.initialize(0, 1 /* one request queue */)?;

        // Read capacity from device config (offset 0, 8 bytes, little-endian).
        let capacity_sectors = Self::read_config_u64(&virtio, 0);

        info!(
            "virtio-blk: capacity = {} sectors ({} MiB)",
            capacity_sectors,
            (capacity_sectors * SECTOR_SIZE as u64) / (1024 * 1024),
        );

        // Allocate request buffer: 2 pages for header + status + data.
        let req_buf = alloc_pages(2, AllocPageFlags::KERNEL)
            .expect("failed to allocate virtio-blk request buffer")
            .as_vaddr();

        Ok(VirtioBlk {
            virtio,
            capacity_sectors,
            req_buf,
            cache: BlockCache::new(),
        })
    }

    fn read_config_u64(virtio: &Virtio, offset: u16) -> u64 {
        let mut value: u64 = 0;
        for i in 0..8 {
            value |= (virtio.read_device_config8(offset + i) as u64) << (i * 8);
        }
        value
    }

    /// Perform a block I/O request (synchronous, spin-waits for completion).
    fn do_request(
        &mut self,
        type_: u32,
        sector: u64,
        num_sectors: usize,
    ) -> Result<(), BlockError> {
        // Write request header.
        let header_ptr = self.req_buf.as_mut_ptr::<VirtioBlkReqHeader>();
        unsafe {
            (*header_ptr).type_ = type_;
            (*header_ptr).reserved = 0;
            (*header_ptr).sector = sector;
        }

        // Clear status byte (sentinel value).
        let status_ptr = self.req_buf.add(16).as_mut_ptr::<u8>();
        unsafe {
            *status_ptr = 0xFF;
        }

        let header_paddr = self.req_buf.as_paddr();
        let status_paddr = self.req_buf.add(16).as_paddr();
        let data_paddr = self.req_buf.add(PAGE_SIZE).as_paddr();
        let data_len = num_sectors * SECTOR_SIZE;

        let data_desc = if type_ == VIRTIO_BLK_T_IN {
            VirtqDescBuffer::WritableFromDevice {
                addr: data_paddr,
                len: data_len,
            }
        } else {
            VirtqDescBuffer::ReadOnlyFromDevice {
                addr: data_paddr,
                len: data_len,
            }
        };

        let chain = &[
            VirtqDescBuffer::ReadOnlyFromDevice {
                addr: header_paddr,
                len: 16,
            },
            data_desc,
            VirtqDescBuffer::WritableFromDevice {
                addr: status_paddr,
                len: 1,
            },
        ];

        let virtq = self.virtio.virtq_mut(VIRTIO_BLK_QUEUE);
        virtq.enqueue(chain);
        virtq.notify();

        // Spin-wait for completion.
        loop {
            if self
                .virtio
                .virtq_mut(VIRTIO_BLK_QUEUE)
                .pop_used()
                .is_some()
            {
                break;
            }
            hint::spin_loop();
        }

        // Check status.
        let status = unsafe { *status_ptr };
        if status != VIRTIO_BLK_S_OK {
            warn!("virtio-blk: request failed with status {}", status);
            return Err(BlockError::IoError);
        }

        Ok(())
    }

    /// Read a single sector directly from the device (bypassing cache).
    fn read_sector_raw(&mut self, sector: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        self.do_request(VIRTIO_BLK_T_IN, sector, 1)?;

        // Copy from data buffer to caller's buffer.
        let data_ptr = self.req_buf.add(PAGE_SIZE).as_ptr::<u8>();
        unsafe {
            core::ptr::copy_nonoverlapping(data_ptr, buf.as_mut_ptr(), SECTOR_SIZE);
        }
        Ok(())
    }

    /// Write a single sector directly to the device (bypassing cache).
    fn write_sector_raw(&mut self, sector: u64, buf: &[u8]) -> Result<(), BlockError> {
        // Copy data to request buffer.
        let data_ptr = self.req_buf.add(PAGE_SIZE).as_mut_ptr::<u8>();
        unsafe {
            core::ptr::copy_nonoverlapping(buf.as_ptr(), data_ptr, SECTOR_SIZE);
        }

        self.do_request(VIRTIO_BLK_T_OUT, sector, 1)
    }

    fn read_sectors_impl(&mut self, start_sector: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        let num_sectors = buf.len() / SECTOR_SIZE;
        for i in 0..num_sectors {
            let sector = start_sector + i as u64;
            let offset = i * SECTOR_SIZE;

            // Check cache first.
            if let Some(cached) = self.cache.get(sector) {
                buf[offset..offset + SECTOR_SIZE].copy_from_slice(cached);
                continue;
            }

            // Cache miss — read from device.
            self.read_sector_raw(sector, &mut buf[offset..offset + SECTOR_SIZE])?;

            // Populate cache.
            self.cache.put(sector, &buf[offset..offset + SECTOR_SIZE]);
        }

        Ok(())
    }

    fn write_sectors_impl(&mut self, start_sector: u64, buf: &[u8]) -> Result<(), BlockError> {
        let num_sectors = buf.len() / SECTOR_SIZE;
        for i in 0..num_sectors {
            let sector = start_sector + i as u64;
            let offset = i * SECTOR_SIZE;

            self.write_sector_raw(sector, &buf[offset..offset + SECTOR_SIZE])?;

            // Write-through: invalidate cache entry.
            self.cache.invalidate(sector);
        }

        Ok(())
    }

    pub fn handle_irq(&mut self) {
        if !self
            .virtio
            .read_isr_status()
            .contains(IsrStatus::QUEUE_INTR)
        {
            return;
        }
        // For synchronous I/O, the IRQ just acknowledges the interrupt.
        // Completions are processed by the spin-wait loop in do_request().
    }
}

/// Thread-safe wrapper around VirtioBlk that implements BlockDevice.
struct VirtioBlockDriver {
    device: Arc<SpinLock<VirtioBlk>>,
}

impl BlockDevice for VirtioBlockDriver {
    fn read_sectors(&self, start_sector: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        self.device.lock().read_sectors_impl(start_sector, buf)
    }

    fn write_sectors(&self, start_sector: u64, buf: &[u8]) -> Result<(), BlockError> {
        self.device.lock().write_sectors_impl(start_sector, buf)
    }

    fn flush(&self) -> Result<(), BlockError> {
        // Write-through cache, nothing to flush.
        Ok(())
    }

    fn capacity_bytes(&self) -> u64 {
        let dev = self.device.lock();
        dev.capacity_sectors * SECTOR_SIZE as u64
    }

    fn sector_size(&self) -> u32 {
        SECTOR_SIZE as u32
    }
}

struct VirtioBlkProber;

impl DeviceProber for VirtioBlkProber {
    #[cfg(target_arch = "x86_64")]
    fn probe_pci(&self, pci_device: &PciDevice) {
        if pci_device.config().vendor_id() != 0x1af4 {
            return;
        }

        // VirtIO block: device ID 0x1001 (transitional) or 0x1042 (modern).
        let device_id = pci_device.config().device_id();
        if device_id != 0x1042 && device_id != 0x1001 {
            return;
        }

        trace!("virtio-blk: found the device (over PCI)");
        let transport = match VirtioModernPci::probe_pci(pci_device) {
            Ok(transport) => transport,
            Err(VirtioAttachError::InvalidVendorId) => return,
            Err(err) => {
                trace!(
                    "failed to attach virtio-blk as modern: {:?}, trying legacy",
                    err
                );
                match VirtioLegacyPci::probe_pci(pci_device) {
                    Ok(transport) => transport,
                    Err(err) => {
                        warn!("failed to attach virtio-blk as legacy: {:?}", err);
                        return;
                    }
                }
            }
        };

        let device = match VirtioBlk::new(transport) {
            Ok(d) => d,
            Err(err) => {
                warn!("failed to initialize virtio-blk: {:?}", err);
                return;
            }
        };

        let device = Arc::new(SpinLock::new(device));
        let driver = Arc::new(VirtioBlockDriver {
            device: device.clone(),
        });

        // Self-test: read, write-readback, restore.
        {
            let cap = driver.capacity_bytes() / SECTOR_SIZE as u64;
            let test_sector = cap - 1; // Use last sector to avoid clobbering fs data.

            // Read first 4 sectors (includes ext2 superblock at offset 1024).
            let mut buf = [0u8; SECTOR_SIZE * 4];
            match driver.read_sectors(0, &mut buf) {
                Ok(()) => {
                    let magic = u16::from_le_bytes([buf[1024 + 56], buf[1024 + 57]]);
                    if magic == 0xEF53 {
                        info!("virtio-blk: read OK (ext2 superblock detected)");
                    } else {
                        info!("virtio-blk: read OK (sector 0 read successfully)");
                    }
                }
                Err(err) => {
                    warn!("virtio-blk: read FAILED: {:?}", err);
                }
            }

            // Write-readback test on last sector.
            let mut original = [0u8; SECTOR_SIZE];
            if driver.read_sectors(test_sector, &mut original).is_ok() {
                let mut pattern = [0u8; SECTOR_SIZE];
                for (i, b) in pattern.iter_mut().enumerate() {
                    *b = (i & 0xFF) as u8;
                }
                if driver.write_sectors(test_sector, &pattern).is_ok() {
                    let mut readback = [0u8; SECTOR_SIZE];
                    if driver.read_sectors(test_sector, &mut readback).is_ok() {
                        if readback == pattern {
                            info!("virtio-blk: write-readback OK");
                        } else {
                            warn!("virtio-blk: write-readback MISMATCH");
                        }
                    }
                    // Restore original sector.
                    let _ = driver.write_sectors(test_sector, &original);
                }
            }
        }

        register_block_device(driver);
        attach_irq(pci_device.config().interrupt_line(), move || {
            device.lock().handle_irq();
        });

        info!("virtio-blk: driver initialized");
    }

    fn probe_virtio_mmio(&self, mmio_device: &VirtioMmioDevice) {
        let mmio = mmio_device.mmio_base.as_vaddr();
        let magic = unsafe { *mmio.as_ptr::<u32>() };
        let virtio_version = unsafe { *mmio.add(4).as_ptr::<u32>() };
        let device_id = unsafe { *mmio.add(8).as_ptr::<u32>() };

        // VirtIO MMIO block device: magic=0x74726976, version=2, device_id=2.
        if magic != 0x74726976 || virtio_version != 2 || device_id != 2 {
            return;
        }

        trace!("virtio-blk: found the device (over MMIO)");

        let transport = Arc::new(VirtioMmio::new(mmio_device.mmio_base));
        let device = match VirtioBlk::new(transport) {
            Ok(d) => Arc::new(SpinLock::new(d)),
            Err(VirtioAttachError::InvalidVendorId) => return,
            Err(err) => {
                warn!("failed to attach virtio-blk: {:?}", err);
                return;
            }
        };

        let driver = Arc::new(VirtioBlockDriver {
            device: device.clone(),
        });

        register_block_device(driver);
        attach_irq(mmio_device.irq, move || {
            device.lock().handle_irq();
        });

        info!("virtio-blk: driver initialized (MMIO)");
    }
}

pub fn init() {
    register_driver_prober(Box::new(VirtioBlkProber));
}
