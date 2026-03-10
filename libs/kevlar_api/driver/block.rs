// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Block device driver API.
use alloc::sync::Arc;
use kevlar_platform::spinlock::SpinLock;

/// Error type for block device operations.
#[derive(Debug)]
pub enum BlockError {
    /// I/O error from the device.
    IoError,
    /// The operation is not supported.
    Unsupported,
}

/// Trait for block devices that can read and write sectors.
pub trait BlockDevice: Send + Sync {
    /// Read sectors starting at `start_sector` into `buf`.
    /// `buf` length must be a multiple of the sector size.
    fn read_sectors(&self, start_sector: u64, buf: &mut [u8]) -> Result<(), BlockError>;

    /// Write sectors starting at `start_sector` from `buf`.
    /// `buf` length must be a multiple of the sector size.
    fn write_sectors(&self, start_sector: u64, buf: &[u8]) -> Result<(), BlockError>;

    /// Flush any cached writes to the device.
    fn flush(&self) -> Result<(), BlockError>;

    /// Total capacity in bytes.
    fn capacity_bytes(&self) -> u64;

    /// Sector size in bytes (usually 512).
    fn sector_size(&self) -> u32;
}

static BLOCK_DEVICE: SpinLock<Option<Arc<dyn BlockDevice>>> = SpinLock::new(None);

/// Register a block device. Only one block device is supported currently.
pub fn register_block_device(device: Arc<dyn BlockDevice>) {
    *BLOCK_DEVICE.lock() = Some(device);
}

/// Get the registered block device, if any.
pub fn block_device() -> Option<Arc<dyn BlockDevice>> {
    BLOCK_DEVICE.lock().clone()
}
