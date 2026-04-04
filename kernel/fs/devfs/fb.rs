// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! /dev/fb0 — Linux framebuffer device.
//!
//! Supports the standard fbdev ioctls (FBIOGET_VSCREENINFO, FBIOGET_FSCREENINFO)
//! and mmap for direct userspace framebuffer access.

use alloc::sync::Arc;
use crate::result::{Errno, Result};
use crate::fs::inode::{FileLike, OpenOptions, PollStatus};
use crate::fs::stat::{Stat, DevId, FileSize};
use crate::user_buffer::{UserBufWriter, UserBufReader, UserBufferMut, UserBuffer};
use core::fmt;

// ─── Linux fbdev ioctl numbers ──────────────────────────────────────────────

const FBIOGET_VSCREENINFO: usize = 0x4600;
const FBIOPUT_VSCREENINFO: usize = 0x4601;
const FBIOGET_FSCREENINFO: usize = 0x4602;
const FBIOBLANK: usize = 0x4611;

// ─── fb_var_screeninfo (160 bytes on Linux x86_64) ──────────────────────────

#[repr(C)]
#[derive(Clone, Copy)]
struct FbVarScreeninfo {
    xres: u32,
    yres: u32,
    xres_virtual: u32,
    yres_virtual: u32,
    xoffset: u32,
    yoffset: u32,
    bits_per_pixel: u32,
    grayscale: u32,
    red_offset: u32,
    red_length: u32,
    red_msb_right: u32,
    green_offset: u32,
    green_length: u32,
    green_msb_right: u32,
    blue_offset: u32,
    blue_length: u32,
    blue_msb_right: u32,
    transp_offset: u32,
    transp_length: u32,
    transp_msb_right: u32,
    nonstd: u32,
    activate: u32,
    height: u32,
    width: u32,
    accel_flags: u32,
    pixclock: u32,
    left_margin: u32,
    right_margin: u32,
    upper_margin: u32,
    lower_margin: u32,
    hsync_len: u32,
    vsync_len: u32,
    sync: u32,
    vmode: u32,
    rotate: u32,
    colorspace: u32,
    reserved: [u32; 4],
}

// ─── fb_fix_screeninfo ──────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy)]
struct FbFixScreeninfo {
    id: [u8; 16],
    smem_start: u64,
    smem_len: u32,
    type_: u32,
    type_aux: u32,
    visual: u32,
    xpanstep: u16,
    ypanstep: u16,
    ywrapstep: u16,
    _pad: u16,
    line_length: u32,
    mmio_start: u64,
    mmio_len: u32,
    accel: u32,
    capabilities: u16,
    reserved: [u16; 2],
    _pad2: u16,
}

// ─── FramebufferFile ────────────────────────────────────────────────────────

pub struct FramebufferFile;

impl FramebufferFile {
    pub fn new() -> Self {
        FramebufferFile
    }
}

impl fmt::Debug for FramebufferFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FramebufferFile(/dev/fb0)")
    }
}

impl FileLike for FramebufferFile {
    fn stat(&self) -> Result<Stat> {
        use crate::fs::stat::{FileMode, S_IFCHR};
        Ok(Stat {
            mode: FileMode::new(S_IFCHR | 0o666),
            rdev: DevId::new((29 << 8) | 0),
            size: FileSize(bochs_fb::size() as isize),
            ..Stat::zeroed()
        })
    }

    fn open(&self, _options: &OpenOptions) -> Result<Option<Arc<dyn FileLike>>> {
        let init = bochs_fb::is_initialized();
        let pid = crate::process::current_process().pid().as_i32();
        info!("fb0: open() pid={} initialized={}", pid, init);
        Ok(None) // use self as the opened file
    }

    fn ioctl(&self, cmd: usize, arg: usize) -> Result<isize> {
        let pid = crate::process::current_process().pid().as_i32();
        info!("fb0: ioctl pid={} cmd={:#x} arg={:#x}", pid, cmd, arg);
        if !bochs_fb::is_initialized() {
            warn!("fb0: ioctl ENODEV — not initialized!");
            return Err(Errno::ENODEV.into());
        }

        match cmd {
            FBIOGET_VSCREENINFO => {
                let w = bochs_fb::width();
                let h = bochs_fb::height();
                let bpp = bochs_fb::bpp();
                let info = FbVarScreeninfo {
                    xres: w, yres: h,
                    xres_virtual: w, yres_virtual: h,
                    xoffset: 0, yoffset: 0,
                    bits_per_pixel: bpp, grayscale: 0,
                    // BGRA 8888
                    red_offset: 16, red_length: 8, red_msb_right: 0,
                    green_offset: 8, green_length: 8, green_msb_right: 0,
                    blue_offset: 0, blue_length: 8, blue_msb_right: 0,
                    transp_offset: 24, transp_length: 8, transp_msb_right: 0,
                    nonstd: 0, activate: 0,
                    height: 0xFFFFFFFF, width: 0xFFFFFFFF,
                    accel_flags: 0, pixclock: 0,
                    left_margin: 0, right_margin: 0,
                    upper_margin: 0, lower_margin: 0,
                    hsync_len: 0, vsync_len: 0,
                    sync: 0, vmode: 0, rotate: 0, colorspace: 0,
                    reserved: [0; 4],
                };
                let uaddr = kevlar_platform::address::UserVAddr::new_nonnull(arg)?;
                uaddr.write::<FbVarScreeninfo>(&info)?;
                Ok(0)
            }
            FBIOPUT_VSCREENINFO => Ok(0), // accept silently
            FBIOGET_FSCREENINFO => {
                let mut id = [0u8; 16];
                id[..9].copy_from_slice(b"Bochs VGA");
                let info = FbFixScreeninfo {
                    id,
                    smem_start: bochs_fb::phys_addr() as u64,
                    smem_len: bochs_fb::size() as u32,
                    type_: 0, type_aux: 0, visual: 2,
                    xpanstep: 0, ypanstep: 0, ywrapstep: 0, _pad: 0,
                    line_length: bochs_fb::stride(),
                    mmio_start: 0, mmio_len: 0, accel: 0,
                    capabilities: 0, reserved: [0; 2], _pad2: 0,
                };
                let uaddr = kevlar_platform::address::UserVAddr::new_nonnull(arg)?;
                uaddr.write::<FbFixScreeninfo>(&info)?;
                Ok(0)
            }
            FBIOBLANK => Ok(0),
            _ => Err(Errno::ENOTTY.into()),
        }
    }

    #[allow(unsafe_code)]
    fn read(&self, offset: usize, buf: UserBufferMut<'_>, _options: &OpenOptions) -> Result<usize> {
        if !bochs_fb::is_initialized() {
            return Err(Errno::ENODEV.into());
        }
        let fb_size = bochs_fb::size();
        if offset >= fb_size {
            return Ok(0);
        }
        let len = buf.len().min(fb_size - offset);
        let fb_vaddr = kevlar_platform::address::PAddr::new(bochs_fb::phys_addr()).as_vaddr();
        // Safety: fb_vaddr points to MMIO framebuffer memory mapped by the PCI BAR.
        let src = unsafe {
            core::slice::from_raw_parts((fb_vaddr.value() + offset) as *const u8, len)
        };
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(src)?;
        Ok(len)
    }

    #[allow(unsafe_code)]
    fn write(&self, offset: usize, buf: UserBuffer<'_>, _options: &OpenOptions) -> Result<usize> {
        if !bochs_fb::is_initialized() {
            return Err(Errno::ENODEV.into());
        }
        let fb_size = bochs_fb::size();
        if offset >= fb_size {
            return Ok(0);
        }
        let len = buf.len().min(fb_size - offset);
        let fb_vaddr = kevlar_platform::address::PAddr::new(bochs_fb::phys_addr()).as_vaddr();
        // Safety: fb_vaddr points to writable MMIO framebuffer memory.
        let dst = unsafe {
            core::slice::from_raw_parts_mut((fb_vaddr.value() + offset) as *mut u8, len)
        };
        let mut reader = UserBufReader::from(buf);
        reader.read_bytes(dst)?;
        Ok(len)
    }

    fn mmap_phys_base(&self) -> Option<usize> {
        if bochs_fb::is_initialized() {
            Some(bochs_fb::phys_addr())
        } else {
            None
        }
    }

    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus::POLLOUT)
    }
}
