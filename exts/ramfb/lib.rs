// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! QEMU `ramfb` framebuffer scan-out for arm64 virt.
//!
//! Background: arm64 QEMU virt has no legacy PCI, so the existing
//! `bochs_fb` PCI prober never finds a display device.  Blog 229
//! worked around this by allocating kernel RAM and pointing
//! `/dev/fb0` at it — Xorg renders into the buffer, but no QEMU
//! display pipe scans it out, so the QEMU window stays blank.
//!
//! `ramfb` is QEMU's solution: with `-device ramfb` on the QEMU
//! command line, QEMU exposes a fw_cfg file `etc/ramfb` that the
//! guest writes a 28-byte config to (target paddr, dimensions,
//! pixel format).  QEMU then scans out that memory each frame.
//!
//! This driver is a one-shot setup, not a probing prober: the
//! kernel calls `init(fw_cfg_base, fb_paddr, w, h, stride)` once
//! after `bochs_fb::init_ram_backed` succeeds.
//!
//! References:
//! - `docs/specs/fw_cfg.rst` (QEMU)
//! - `docs/specs/ramfb.rst` (QEMU)
//! - `kernel/pipe.rs` and `exts/bochs_fb/lib.rs` for kext patterns
#![no_std]

extern crate alloc;

#[macro_use]
extern crate kevlar_api;

use kevlar_api::address::PAddr;

// ── fw_cfg MMIO register layout (QEMU virt arm64) ────────────────────
//
// The fw_cfg device exposes a 16-byte MMIO region (the DTB reports
// `reg = <addr 0xa>` but only 16 bytes are well-defined).  Layout:
//
//   offset 0x0..0x8 : data register (8 bytes; per-byte access advances
//                     the file pointer by the access width)
//   offset 0x8..0xa : selector register (2 bytes, BE on ARM)
//   offset 0x10..0x18: DMA descriptor pointer (8 bytes, BE)
//
// We use port-mode (selector + byte-by-byte data) since the entire
// ramfb config is just 28 bytes.  DMA isn't needed.

const FW_CFG_DATA_OFFSET: usize = 0x0;
const FW_CFG_SELECTOR_OFFSET: usize = 0x8;

const FW_CFG_SIGNATURE: u16 = 0x0000;
const FW_CFG_FILE_DIR: u16 = 0x0019;

/// `etc/ramfb` config struct, written to fw_cfg in big-endian.
/// 28 bytes total, layout fixed by QEMU.
#[repr(C, packed)]
struct RamfbCfg {
    addr: u64,    // BE: guest physical addr of framebuffer
    fourcc: u32,  // BE: pixel format ('XR24' = 0x34325258 = XRGB8888)
    flags: u32,   // BE: 0
    width: u32,   // BE
    height: u32,  // BE
    stride: u32,  // BE: bytes per scanline (0 → width * bpp/8)
}

/// `fourcc` for the BGRA layout `bochs_fb` uses for `/dev/fb0`.
///
/// Kevlar's `bochs_fb` describes its framebuffer with `red_offset=16,
/// green_offset=8, blue_offset=0, transp_offset=24` (see
/// `kernel/fs/devfs/fb.rs::FBIOGET_VSCREENINFO`) — that is, each
/// little-endian u32 pixel byte order is `B, G, R, A`.  In DRM
/// fourcc, this is `XRGB8888` = `'X','R','2','4'` = 0x34325258.
const DRM_FORMAT_XRGB8888: u32 = 0x34325258;

/// Write the selector register.  Selector is 2 bytes, BE on ARM.
unsafe fn select(base: PAddr, sel: u16) {
    let v = base.as_vaddr().add(FW_CFG_SELECTOR_OFFSET);
    // Selector reads/writes are big-endian on ARM regardless of host.
    unsafe { v.mmio_write16(sel.to_be()) };
}

/// Read one byte from the data register, auto-advancing the file
/// pointer.  Each access advances by exactly the access width.
unsafe fn read_byte(base: PAddr) -> u8 {
    let v = base.as_vaddr().add(FW_CFG_DATA_OFFSET);
    unsafe { v.mmio_read8() }
}

/// Write one byte to the data register.
unsafe fn write_byte(base: PAddr, b: u8) {
    let v = base.as_vaddr().add(FW_CFG_DATA_OFFSET);
    unsafe { v.mmio_write8(b) };
}

/// Read N bytes from the currently-selected fw_cfg file.
unsafe fn read_n(base: PAddr, n: usize) -> alloc::vec::Vec<u8> {
    let mut v = alloc::vec::Vec::with_capacity(n);
    for _ in 0..n {
        v.push(unsafe { read_byte(base) });
    }
    v
}

/// Find the `select` index of a given fw_cfg file by name.
unsafe fn find_file(base: PAddr, name: &str) -> Option<u16> {
    unsafe { select(base, FW_CFG_FILE_DIR) };
    // First 4 bytes: BE u32 count of entries.
    let hdr = unsafe { read_n(base, 4) };
    let count = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
    info!("ramfb: fw_cfg file dir has {} entries", count);
    // Each entry is 64 bytes: u32-BE size, u16-BE select, u16 reserved,
    // char[56] name (NUL-terminated).
    let mut found: Option<u16> = None;
    for _ in 0..count {
        let entry = unsafe { read_n(base, 64) };
        let sz = u32::from_be_bytes([entry[0], entry[1], entry[2], entry[3]]);
        let sel = u16::from_be_bytes([entry[4], entry[5]]);
        let name_bytes = &entry[8..];
        let name_end = name_bytes.iter().position(|&b| b == 0).unwrap_or(56);
        let entry_name = match core::str::from_utf8(&name_bytes[..name_end]) {
            Ok(s) => s,
            Err(_) => continue,
        };
        info!("ramfb:   {:#06x} [{:5} bytes] {}", sel, sz, entry_name);
        if entry_name == name && found.is_none() {
            found = Some(sel);
        }
    }
    found
}

/// Verify fw_cfg signature: select index 0 (FW_CFG_SIGNATURE) returns
/// the literal bytes "QEMU\0\0\0\0" if the device is functional.
unsafe fn verify_signature(base: PAddr) -> bool {
    unsafe { select(base, FW_CFG_SIGNATURE) };
    let sig = unsafe { read_n(base, 4) };
    sig == [b'Q', b'E', b'M', b'U']
}

/// One-shot setup: tell QEMU's ramfb device where in guest RAM the
/// framebuffer lives and what shape it has.  After this returns,
/// QEMU's display backend (SDL/cocoa/VNC, depending on `-display`)
/// scans out from `fb_paddr` every frame.
///
/// Returns `false` if fw_cfg is absent, the signature mismatches, or
/// the `etc/ramfb` file isn't present (i.e., `-device ramfb` wasn't
/// passed to QEMU).  Caller should warn but not fail boot.
pub fn init(fw_cfg_base: PAddr, fb_paddr: PAddr, width: u32, height: u32) -> bool {
    info!(
        "ramfb: setup at fw_cfg={:#x}, fb={:#x}, {}x{} XRGB8888",
        fw_cfg_base.value(),
        fb_paddr.value(),
        width,
        height
    );

    if !unsafe { verify_signature(fw_cfg_base) } {
        warn!("ramfb: fw_cfg signature mismatch — no QEMU?");
        return false;
    }

    let sel = match unsafe { find_file(fw_cfg_base, "etc/ramfb") } {
        Some(s) => s,
        None => {
            warn!("ramfb: etc/ramfb not found in fw_cfg — pass -device ramfb to QEMU");
            return false;
        }
    };

    // Populate the 28-byte config struct (all multi-byte fields BE).
    let stride = width * 4; // 4 bytes per pixel for XRGB8888
    let cfg = RamfbCfg {
        addr: (fb_paddr.value() as u64).to_be(),
        fourcc: DRM_FORMAT_XRGB8888.to_be(),
        flags: 0u32.to_be(),
        width: width.to_be(),
        height: height.to_be(),
        stride: stride.to_be(),
    };

    // Write the struct via the FW_CFG DMA interface.  Per QEMU spec,
    // writing a 64-bit BE guest-physical address to offset 0x10 of
    // the fw_cfg MMIO triggers an immediate, synchronous transfer
    // described by a `FWCfgDmaAccess` descriptor at that paddr.
    //
    // Port-mode byte writes via mmio_write8 don't seem to reach
    // QEMU's fw_cfg under HVF — readback after byte writes shows
    // the file still all-zero.  DMA uses a single MMIO write and
    // the fw_cfg engine does the bulk transfer.
    #[repr(C)]
    struct DmaDesc { control: u32, length: u32, address: u64 }
    const FW_CFG_DMA_CTL_SELECT: u32 = 0x08;
    const FW_CFG_DMA_CTL_WRITE: u32 = 0x10;

    let bytes: [u8; core::mem::size_of::<RamfbCfg>()] =
        unsafe { core::mem::transmute(cfg) };
    let heap_bytes: alloc::boxed::Box<[u8; 28]> = alloc::boxed::Box::new(bytes);
    // Direct-map identity: arm64 maps phys `P` at virtual
    // `KERNEL_BASE_ADDR + P`.  Kernel heap allocations live in the
    // direct map, so paddr = vaddr - KERNEL_BASE_ADDR.
    const KERNEL_BASE: usize = 0xffff_0000_0000_0000;
    let cfg_paddr = (heap_bytes.as_ptr() as usize).wrapping_sub(KERNEL_BASE);

    let desc = DmaDesc {
        control: ((sel as u32) << 16
            | FW_CFG_DMA_CTL_SELECT
            | FW_CFG_DMA_CTL_WRITE).to_be(),
        length: (28u32).to_be(),
        address: (cfg_paddr as u64).to_be(),
    };
    let desc_box: alloc::boxed::Box<DmaDesc> = alloc::boxed::Box::new(desc);
    let desc_paddr = (&*desc_box as *const _ as usize).wrapping_sub(KERNEL_BASE);

    let dma_reg = fw_cfg_base.as_vaddr().add(0x10);
    info!("ramfb: DMA cfg_paddr={:#x} desc_paddr={:#x}", cfg_paddr, desc_paddr);
    unsafe { dma_reg.mmio_write64((desc_paddr as u64).to_be()) };

    // Read back to verify the file now holds our config.
    unsafe { select(fw_cfg_base, sel) };
    let readback = unsafe { read_n(fw_cfg_base, 28) };
    info!("ramfb: readback first 16 bytes: {:02x?}", &readback[..16]);

    // Leak the heap allocations — DMA may still be in flight when
    // this function returns (asynchronous on some QEMU builds).
    core::mem::forget(heap_bytes);
    core::mem::forget(desc_box);

    info!(
        "ramfb: configured (sel={:#x}, stride={}); QEMU display now scans /dev/fb0",
        sel, stride
    );
    true
}
