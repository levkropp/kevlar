// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Bochs VGA framebuffer driver for QEMU's default display adapter.
//!
//! Detects the Bochs VGA device (vendor 0x1234, device 0x1111) on PCI,
//! programs the VBE registers to set a linear framebuffer mode, and
//! exposes the framebuffer metadata for /dev/fb0.
#![no_std]

extern crate alloc;

#[macro_use]
extern crate kevlar_api;

use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use kevlar_api::address::PAddr;
#[cfg(target_arch = "x86_64")]
use kevlar_api::driver::ioport::IoPort;
use kevlar_api::driver::{register_driver_prober, DeviceProber};
#[cfg(target_arch = "x86_64")]
use kevlar_api::driver::pci::{Bar, PciDevice};
use kevlar_api::driver::VirtioMmioDevice;

// ─── VBE Registers (Bochs VGA) ──────────────────────────────────────────────

const VBE_DISPI_IOPORT_INDEX: u16 = 0x01CE;
const VBE_DISPI_IOPORT_DATA: u16 = 0x01CF;

const VBE_DISPI_INDEX_ID: u16 = 0x00;
const VBE_DISPI_INDEX_XRES: u16 = 0x01;
const VBE_DISPI_INDEX_YRES: u16 = 0x02;
const VBE_DISPI_INDEX_BPP: u16 = 0x03;
const VBE_DISPI_INDEX_ENABLE: u16 = 0x04;
const VBE_DISPI_INDEX_BANK: u16 = 0x05;
const VBE_DISPI_INDEX_VIRT_WIDTH: u16 = 0x06;
const VBE_DISPI_INDEX_VIRT_HEIGHT: u16 = 0x07;
const VBE_DISPI_INDEX_X_OFFSET: u16 = 0x08;
const VBE_DISPI_INDEX_Y_OFFSET: u16 = 0x09;
const VBE_DISPI_INDEX_VIDEO_MEMORY_64K: u16 = 0x0A;

const VBE_DISPI_DISABLED: u16 = 0x00;
const VBE_DISPI_ENABLED: u16 = 0x01;
const VBE_DISPI_LFB_ENABLED: u16 = 0x40;

// ─── Default mode ───────────────────────────────────────────────────────────

const DEFAULT_WIDTH: u16 = 1024;
const DEFAULT_HEIGHT: u16 = 768;
const DEFAULT_BPP: u16 = 32;

// ─── Framebuffer global state ───────────────────────────────────────────────

static FB_INITIALIZED: AtomicBool = AtomicBool::new(false);
static FB_PADDR: AtomicUsize = AtomicUsize::new(0);
static FB_SIZE: AtomicUsize = AtomicUsize::new(0);
static FB_WIDTH: AtomicU32 = AtomicU32::new(0);
static FB_HEIGHT: AtomicU32 = AtomicU32::new(0);
static FB_STRIDE: AtomicU32 = AtomicU32::new(0);
static FB_BPP: AtomicU32 = AtomicU32::new(0);

/// Returns true if the framebuffer has been initialized.
pub fn is_initialized() -> bool {
    FB_INITIALIZED.load(Ordering::Relaxed)
}

/// Physical address of the linear framebuffer.
pub fn phys_addr() -> usize {
    FB_PADDR.load(Ordering::Relaxed)
}

/// Total framebuffer size in bytes.
pub fn size() -> usize {
    FB_SIZE.load(Ordering::Relaxed)
}

/// Width in pixels.
pub fn width() -> u32 {
    FB_WIDTH.load(Ordering::Relaxed)
}

/// Height in pixels.
pub fn height() -> u32 {
    FB_HEIGHT.load(Ordering::Relaxed)
}

/// Bytes per scanline.
pub fn stride() -> u32 {
    FB_STRIDE.load(Ordering::Relaxed)
}

/// Bits per pixel.
pub fn bpp() -> u32 {
    FB_BPP.load(Ordering::Relaxed)
}

// ─── VBE Register Access ────────────────────────────────────────────────────

#[cfg(target_arch = "x86_64")]
fn vbe_write(index: u16, value: u16) {
    let port = IoPort::new(VBE_DISPI_IOPORT_INDEX);
    port.write16(0, index);
    let data = IoPort::new(VBE_DISPI_IOPORT_DATA);
    data.write16(0, value);
}

#[cfg(target_arch = "x86_64")]
fn vbe_read(index: u16) -> u16 {
    let port = IoPort::new(VBE_DISPI_IOPORT_INDEX);
    port.write16(0, index);
    let data = IoPort::new(VBE_DISPI_IOPORT_DATA);
    data.read16(0)
}

/// Program the Bochs VBE registers to set a linear framebuffer mode.
#[cfg(target_arch = "x86_64")]
fn set_mode(width: u16, height: u16, bpp: u16) {
    // Disable display first
    vbe_write(VBE_DISPI_INDEX_ENABLE, VBE_DISPI_DISABLED);

    // Set resolution and color depth
    vbe_write(VBE_DISPI_INDEX_XRES, width);
    vbe_write(VBE_DISPI_INDEX_YRES, height);
    vbe_write(VBE_DISPI_INDEX_BPP, bpp);
    vbe_write(VBE_DISPI_INDEX_VIRT_WIDTH, width);
    vbe_write(VBE_DISPI_INDEX_VIRT_HEIGHT, height);
    vbe_write(VBE_DISPI_INDEX_X_OFFSET, 0);
    vbe_write(VBE_DISPI_INDEX_Y_OFFSET, 0);
    vbe_write(VBE_DISPI_INDEX_BANK, 0);

    // Enable with linear framebuffer
    vbe_write(VBE_DISPI_INDEX_ENABLE, VBE_DISPI_ENABLED | VBE_DISPI_LFB_ENABLED);
}

// ─── PCI Probe ──────────────────────────────────────────────────────────────

struct BochsFbProber;

impl DeviceProber for BochsFbProber {
    #[cfg(target_arch = "x86_64")]
    fn probe_pci(&self, pci_device: &PciDevice) {
        // Bochs VGA: vendor 0x1234, device 0x1111
        if pci_device.config().vendor_id() != 0x1234 {
            return;
        }
        if pci_device.config().device_id() != 0x1111 {
            return;
        }

        info!("bochs-fb: found Bochs VGA device on PCI {}:{}",
              pci_device.bus(), pci_device.slot());

        // BAR0 = linear framebuffer (memory-mapped)
        let bar0 = pci_device.config().bar(0);
        let fb_paddr = match bar0 {
            Bar::MemoryMapped { paddr } => paddr,
            Bar::IOMapped { port } => {
                warn!("bochs-fb: BAR0 is I/O mapped (port={:#x}), expected MMIO", port);
                return;
            }
        };

        // Read video memory size from VBE register (in 64KB blocks)
        let vram_64k = vbe_read(VBE_DISPI_INDEX_VIDEO_MEMORY_64K) as usize;
        let vram_size = if vram_64k > 0 {
            vram_64k * 64 * 1024
        } else {
            // Fallback: assume 16MB (QEMU default for -vga std)
            16 * 1024 * 1024
        };

        // Check VBE ID to confirm the device is responsive
        let vbe_id = vbe_read(VBE_DISPI_INDEX_ID);
        info!("bochs-fb: VBE ID={:#06x}, VRAM={}MB at paddr {:#x}",
              vbe_id, vram_size / (1024 * 1024), fb_paddr.value());

        // Set the default framebuffer mode
        set_mode(DEFAULT_WIDTH, DEFAULT_HEIGHT, DEFAULT_BPP);

        // Verify the mode was set
        let actual_w = vbe_read(VBE_DISPI_INDEX_XRES);
        let actual_h = vbe_read(VBE_DISPI_INDEX_YRES);
        let actual_bpp = vbe_read(VBE_DISPI_INDEX_BPP);
        info!("bochs-fb: mode set to {}x{}x{}", actual_w, actual_h, actual_bpp);

        let stride = (actual_w as u32) * (actual_bpp as u32 / 8);
        let fb_size = (stride * actual_h as u32) as usize;

        // Store framebuffer info in globals
        FB_PADDR.store(fb_paddr.value(), Ordering::Relaxed);
        FB_SIZE.store(vram_size.min(fb_size), Ordering::Relaxed);
        FB_WIDTH.store(actual_w as u32, Ordering::Relaxed);
        FB_HEIGHT.store(actual_h as u32, Ordering::Relaxed);
        FB_STRIDE.store(stride, Ordering::Relaxed);
        FB_BPP.store(actual_bpp as u32, Ordering::Relaxed);
        FB_INITIALIZED.store(true, Ordering::Release);

        // Paint the framebuffer blue to confirm it works
        let fb_vaddr = fb_paddr.as_vaddr().value() as *mut u32;
        let pixels = (actual_w as usize) * (actual_h as usize);
        for i in 0..pixels {
            // BGRA: 0xAARRGGBB — dark blue background
            unsafe { fb_vaddr.add(i).write_volatile(0xFF1A1A2E) };
        }

        info!("bochs-fb: framebuffer initialized (painted test pattern)");
    }

    fn probe_virtio_mmio(&self, _mmio_device: &VirtioMmioDevice) {
        // Bochs VGA is PCI only
    }
}

// ─── Public API ─────────────────────────────────────────────────────────────

pub fn init() {
    info!("kext: Loading bochs_fb...");
    register_driver_prober(Box::new(BochsFbProber));
}
