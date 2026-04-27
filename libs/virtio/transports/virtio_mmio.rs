// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::device::IsrStatus;

use kevlar_api::address::{PAddr, VAddr};

use super::VirtioTransport;

pub struct VirtioMmio {
    mmio_base: VAddr,
}

impl VirtioMmio {
    pub fn new(mmio_base: PAddr) -> VirtioMmio {
        VirtioMmio {
            mmio_base: mmio_base.as_vaddr(),
        }
    }
}

// All MMIO register I/O goes through `VAddr::mmio_read*` / `mmio_write*`
// rather than `read_volatile` / `write_volatile` — on aarch64 those use
// explicit inline asm with plain `ldr/str w/x, [x]` so HVF's EL2 data
// abort handler always sees a valid ESR_EL2.ISV.  The generic volatile
// path can be lowered to instruction forms (post-indexed, vector,
// stp/ldp) that clear ISV and panic QEMU's `hvf_handle_exception`.
// See `platform/address.rs::mmio_read32`.

impl VirtioTransport for VirtioMmio {
    fn is_modern(&self) -> bool {
        true
    }

    fn read_device_config8(&self, offset: u16) -> u8 {
        unsafe { self.mmio_base.add((0x100 + offset) as usize).mmio_read8() }
    }

    fn read_isr_status(&self) -> IsrStatus {
        // Per virtio-mmio v1.1 §4.2.2.1, InterruptStatus (0x60) is
        // read-only and InterruptACK (0x64) is write-only.  The device
        // latches its ISR bits and the driver MUST write the bits back
        // to 0x64 to clear them.  Without the ack, the ISR bit stays
        // set and the device cannot raise a new edge on the same bit
        // — sparse-traffic devices like virtio-input fire exactly one
        // interrupt and then go silent.  (High-rate devices like
        // virtio-net/blk hide this because GIC level-triggered re-
        // delivery papers over the missing ack on every busy poll.)
        let bits = unsafe { self.mmio_base.add(0x60).mmio_read32() };
        if bits != 0 {
            unsafe { self.mmio_base.add(0x64).mmio_write32(bits) };
        }
        IsrStatus::from_bits(bits as u8).unwrap()
    }

    fn read_device_status(&self) -> u8 {
        unsafe { self.mmio_base.add(0x70).mmio_read32() as u8 }
    }

    fn write_device_status(&self, value: u8) {
        unsafe {
            self.mmio_base.add(0x70).mmio_write32(value as u32);
        }
    }

    fn read_device_features(&self) -> u64 {
        unsafe {
            self.mmio_base.add(0x14).mmio_write32(0);
            let low = self.mmio_base.add(0x10).mmio_read32();
            self.mmio_base.add(0x14).mmio_write32(1);
            let high = self.mmio_base.add(0x10).mmio_read32();
            ((high as u64) << 32) | (low as u64)
        }
    }

    fn write_driver_features(&self, value: u64) {
        unsafe {
            self.mmio_base.add(0x24).mmio_write32(0);
            self.mmio_base
                .add(0x20)
                .mmio_write32((value & 0xffff_ffff) as u32);
            self.mmio_base.add(0x24).mmio_write32(1);
            self.mmio_base.add(0x20).mmio_write32((value >> 32) as u32);
        }
    }

    fn select_queue(&self, index: u16) {
        unsafe {
            self.mmio_base.add(0x30).mmio_write32(index as u32);
        }
    }

    fn queue_max_size(&self) -> u16 {
        unsafe { self.mmio_base.add(0x34).mmio_read32() as u16 }
    }

    fn set_queue_size(&self, queue_size: u16) {
        unsafe { self.mmio_base.add(0x38).mmio_write32(queue_size as u32) }
    }

    fn notify_queue(&self, index: u16) {
        unsafe {
            self.mmio_base.add(0x50).mmio_write32(index as u32);
        }
    }

    fn enable_queue(&self) {
        unsafe {
            self.mmio_base.add(0x44).mmio_write32(1);
        }
    }

    fn set_queue_desc_paddr(&self, paddr: PAddr) {
        unsafe {
            self.mmio_base
                .add(0x80)
                .mmio_write32((paddr.value() & 0xffff_ffff) as u32);
            self.mmio_base
                .add(0x84)
                .mmio_write32((paddr.value() >> 32) as u32);
        }
    }

    fn set_queue_device_paddr(&self, paddr: PAddr) {
        unsafe {
            self.mmio_base
                .add(0xa0)
                .mmio_write32((paddr.value() & 0xffff_ffff) as u32);
            self.mmio_base
                .add(0xa4)
                .mmio_write32((paddr.value() >> 32) as u32);
        }
    }

    fn set_queue_driver_paddr(&self, paddr: PAddr) {
        unsafe {
            self.mmio_base
                .add(0x90)
                .mmio_write32((paddr.value() & 0xffff_ffff) as u32);
            self.mmio_base
                .add(0x94)
                .mmio_write32((paddr.value() >> 32) as u32);
        }
    }
}
