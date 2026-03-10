// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Device driver APIs.
use crate::kernel_ops::kernel_ops;

use alloc::vec::Vec;

pub mod block;
#[cfg(target_arch = "x86_64")]
pub mod ioport;
pub mod net;
#[cfg(target_arch = "x86_64")]
pub mod pci;

pub use kevlar_platform::bootinfo::VirtioMmioDevice;

use alloc::boxed::Box;
use kevlar_platform::{bootinfo::AllowedPciDevice, spinlock::SpinLock};

#[cfg(target_arch = "x86_64")]
use self::pci::PciDevice;

static DEVICE_PROBERS: SpinLock<Vec<Box<dyn DeviceProber>>> = SpinLock::new(Vec::new());

pub trait Driver: Send + Sync {
    fn name(&self) -> &str;
}

pub trait DeviceProber: Send + Sync {
    #[cfg(target_arch = "x86_64")]
    fn probe_pci(&self, pci_device: &PciDevice);
    fn probe_virtio_mmio(&self, mmio_device: &VirtioMmioDevice);
}

pub fn register_driver_prober(driver: Box<dyn DeviceProber>) {
    DEVICE_PROBERS.lock().push(driver);
}

pub fn attach_irq<F: FnMut() + Send + Sync + 'static>(irq: u8, f: F) {
    kernel_ops().attach_irq(irq, Box::new(f))
}

pub fn init(
    pci_enabled: bool,
    pci_allowlist: &[AllowedPciDevice],
    mmio_devices: &[VirtioMmioDevice],
) {
    // Scan PCI devices (x86 only — ARM64 uses device tree).
    #[cfg(target_arch = "x86_64")]
    if pci_enabled {
        for device in pci::enumerate_pci_devices() {
            if !pci_allowlist.is_empty()
                && !pci_allowlist
                    .iter()
                    .any(|e| e.bus == device.bus() && e.slot == device.slot())
            {
                trace!(
                    "pci: skipping not allowed device: id={:04x}:{:04x}",
                    device.config().vendor_id(),
                    device.config().device_id(),
                );
                continue;
            }

            trace!(
                "pci: found a device: id={:04x}:{:04x}, bar0={:016x?}, irq={}",
                device.config().vendor_id(),
                device.config().device_id(),
                device.config().bar(0),
                device.config().interrupt_line()
            );

            for prober in DEVICE_PROBERS.lock().iter() {
                prober.probe_pci(&device);
            }
        }
    }

    #[cfg(not(target_arch = "x86_64"))]
    let _ = (pci_enabled, pci_allowlist);

    // Register Virtio devices connected over MMIO.
    for device in mmio_devices {
        for prober in DEVICE_PROBERS.lock().iter() {
            prober.probe_virtio_mmio(device);
        }
    }
}
