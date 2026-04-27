// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::device::IsrStatus;

use kevlar_api::address::PAddr;

pub mod virtio_mmio;
#[cfg(target_arch = "x86_64")]
pub mod virtio_pci_legacy;
#[cfg(target_arch = "x86_64")]
pub mod virtio_pci_modern;

pub trait VirtioTransport: Send + Sync {
    fn is_modern(&self) -> bool;
    fn read_device_config8(&self, offset: u16) -> u8;
    /// Write one byte into the device-specific config space.  Only the
    /// MMIO transport exposes this — PCI is read-only on most config
    /// fields.  Default is a no-op so PCI-only callers still build.
    fn write_device_config8(&self, _offset: u16, _value: u8) {}
    fn read_isr_status(&self) -> IsrStatus;
    fn read_device_status(&self) -> u8;
    fn write_device_status(&self, value: u8);
    fn read_device_features(&self) -> u64;
    fn write_driver_features(&self, value: u64);
    fn select_queue(&self, index: u16);
    fn queue_max_size(&self) -> u16;
    fn set_queue_size(&self, queue_size: u16);
    fn notify_queue(&self, index: u16);
    fn enable_queue(&self);
    fn set_queue_desc_paddr(&self, paddr: PAddr);
    fn set_queue_driver_paddr(&self, paddr: PAddr);
    fn set_queue_device_paddr(&self, paddr: PAddr);
}

#[derive(Debug)]
pub enum VirtioAttachError {
    InvalidVendorId,
    MissingFeatures,
    MissingPciCommonCfg,
    MissingPciDeviceCfg,
    MissingPciIsrCfg,
    MissingPciNotifyCfg,
    FeatureNegotiationFailure,
    NotSupportedBarType,
}
