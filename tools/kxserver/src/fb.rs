// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(dead_code)]
//
// Framebuffer access.
//
// Two back-ends:
//
// 1. **Real fbdev**: open `/dev/fb0`, ioctl `FBIOGET_VSCREENINFO` +
//    `FBIOGET_FSCREENINFO`, mmap `smem_len` bytes.  Used on Kevlar
//    where Bochs VGA exposes a 1024x768x32 framebuffer.
//
// 2. **Shadow**: `Vec<u32>` sized to 1024x768.  Used on the native
//    Linux host where `/dev/fb0` usually does not exist (or belongs
//    to the console).  The test harness can dump the buffer to a
//    PPM file at the end of a run for visual inspection.
//
// Pixel format is always little-endian 32-bit `0x00RRGGBB` (the X11
// TrueColor visual we advertise in `setup.rs`).  Stride is
// `stride_px * 4` bytes.

use std::os::fd::{AsRawFd, OwnedFd};

use crate::log;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct FbVarScreenInfo {
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

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FbFixScreenInfo {
    id: [u8; 16],
    smem_start: u64,
    smem_len: u32,
    ty: u32,
    type_aux: u32,
    visual: u32,
    xpanstep: u16,
    ypanstep: u16,
    ywrapstep: u16,
    pad_: u16,
    line_length: u32,
    mmio_start: u64,
    mmio_len: u32,
    accel: u32,
    capabilities: u16,
    reserved: [u16; 2],
}

impl Default for FbFixScreenInfo {
    fn default() -> Self {
        FbFixScreenInfo {
            id: [0; 16],
            smem_start: 0,
            smem_len: 0,
            ty: 0,
            type_aux: 0,
            visual: 0,
            xpanstep: 0,
            ypanstep: 0,
            ywrapstep: 0,
            pad_: 0,
            line_length: 0,
            mmio_start: 0,
            mmio_len: 0,
            accel: 0,
            capabilities: 0,
            reserved: [0; 2],
        }
    }
}

const FBIOGET_VSCREENINFO: libc::c_ulong = 0x4600;
const FBIOGET_FSCREENINFO: libc::c_ulong = 0x4602;

/// Backing store for the framebuffer.
enum Backing {
    /// mmap'd `/dev/fb0`.  We keep the fd alive so the mapping stays valid.
    Mmap { _fd: OwnedFd, ptr: *mut u8, length: usize },
    /// Shadow buffer — `Vec<u32>` owned by the process, no kernel fb.
    Shadow { pixels: Vec<u32> },
}

// Backing contains a raw pointer for the Mmap variant; we only ever use
// it from the single-threaded server so Send/Sync are safe.
unsafe impl Send for Backing {}
unsafe impl Sync for Backing {}

pub struct Framebuffer {
    pub width:     u16,
    pub height:    u16,
    pub stride_px: usize,
    backing:       Backing,
}

impl Framebuffer {
    /// Try to open `/dev/fb0`.  If that fails, allocate a shadow buffer
    /// of the given screen size (typically `setup::SCREEN_WIDTH/HEIGHT`).
    pub fn open(fallback_width: u16, fallback_height: u16) -> Self {
        match Self::try_open_fbdev() {
            Ok(fb) => fb,
            Err(e) => {
                log::warn(format_args!(
                    "framebuffer: /dev/fb0 unavailable ({e}); using shadow buffer {}x{}",
                    fallback_width, fallback_height,
                ));
                Self::new_shadow(fallback_width, fallback_height)
            }
        }
    }

    fn try_open_fbdev() -> Result<Self, String> {
        let c_path = c"/dev/fb0";
        let fd_raw = unsafe { libc::open(c_path.as_ptr(), libc::O_RDWR) };
        if fd_raw < 0 {
            let e = unsafe { *libc::__errno_location() };
            return Err(format!("open /dev/fb0 errno={e}"));
        }
        let fd = unsafe { OwnedFd::from_raw_fd(fd_raw) };

        let mut vinfo: FbVarScreenInfo = Default::default();
        let rc = unsafe {
            libc::ioctl(
                fd.as_raw_fd(),
                FBIOGET_VSCREENINFO as _,
                &mut vinfo as *mut _ as *mut _,
            )
        };
        if rc < 0 {
            let e = unsafe { *libc::__errno_location() };
            return Err(format!("FBIOGET_VSCREENINFO errno={e}"));
        }

        let mut finfo: FbFixScreenInfo = Default::default();
        let rc = unsafe {
            libc::ioctl(
                fd.as_raw_fd(),
                FBIOGET_FSCREENINFO as _,
                &mut finfo as *mut _ as *mut _,
            )
        };
        if rc < 0 {
            let e = unsafe { *libc::__errno_location() };
            return Err(format!("FBIOGET_FSCREENINFO errno={e}"));
        }

        if vinfo.bits_per_pixel != 32 {
            return Err(format!(
                "unsupported bpp {} (only 32bpp BGRA supported)",
                vinfo.bits_per_pixel
            ));
        }

        let length = finfo.smem_len as usize;
        let ptr = unsafe {
            libc::mmap(
                core::ptr::null_mut(),
                length,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            let e = unsafe { *libc::__errno_location() };
            return Err(format!("mmap errno={e}"));
        }

        let stride_px = (finfo.line_length / 4) as usize;
        log::info(format_args!(
            "framebuffer /dev/fb0: {}x{}, {}bpp, stride={}px, mmap={} bytes",
            vinfo.xres, vinfo.yres, vinfo.bits_per_pixel, stride_px, length,
        ));

        Ok(Framebuffer {
            width: vinfo.xres as u16,
            height: vinfo.yres as u16,
            stride_px,
            backing: Backing::Mmap {
                _fd: fd,
                ptr: ptr as *mut u8,
                length,
            },
        })
    }

    fn new_shadow(width: u16, height: u16) -> Self {
        let stride_px = width as usize;
        let pixels = vec![0u32; stride_px * height as usize];
        log::info(format_args!(
            "framebuffer: shadow {}x{} ({} bytes)",
            width, height, pixels.len() * 4
        ));
        Framebuffer {
            width,
            height,
            stride_px,
            backing: Backing::Shadow { pixels },
        }
    }

    /// Raw mutable slice of the entire framebuffer as u32 pixels.
    /// The slice length is `stride_px * height` — stride may exceed width
    /// on real hardware (padding for alignment).
    pub fn pixels_mut(&mut self) -> &mut [u32] {
        match &mut self.backing {
            Backing::Mmap { ptr, length, .. } => {
                let count = *length / 4;
                unsafe { core::slice::from_raw_parts_mut(*ptr as *mut u32, count) }
            }
            Backing::Shadow { pixels } => pixels.as_mut_slice(),
        }
    }

    /// Immutable view of the framebuffer pixels.  Used by CopyArea
    /// source reads that hold `&ServerState`.
    pub fn pixels_read(&self) -> &[u32] {
        match &self.backing {
            Backing::Mmap { ptr, length, .. } => {
                let count = *length / 4;
                unsafe { core::slice::from_raw_parts(*ptr as *const u32, count) }
            }
            Backing::Shadow { pixels } => pixels.as_slice(),
        }
    }

    /// Fill a rectangle with a solid color.  All coordinates are in
    /// absolute screen pixels and must already be clipped to the
    /// framebuffer bounds.
    pub fn fill_rect(&mut self, x: u16, y: u16, w: u16, h: u16, color: u32) {
        let stride_px = self.stride_px;
        let w = w as usize;
        let pix = self.pixels_mut();
        for row in 0..h as usize {
            let start = (y as usize + row) * stride_px + x as usize;
            for i in 0..w {
                pix[start + i] = color;
            }
        }
    }

    /// Write a single pixel (bounds must already be clipped).
    pub fn put_pixel(&mut self, x: u16, y: u16, color: u32) {
        let stride_px = self.stride_px;
        let pix = self.pixels_mut();
        pix[y as usize * stride_px + x as usize] = color;
    }

    /// Copy a rectangle of source pixels into the framebuffer at (dx, dy).
    /// `src` is row-major, `src_stride` pixels per row.  Caller must
    /// ensure that the destination is within bounds.
    pub fn blit(&mut self, dx: u16, dy: u16, w: u16, h: u16, src: &[u32], src_stride: usize) {
        let stride_px = self.stride_px;
        let pix = self.pixels_mut();
        for row in 0..h as usize {
            let dst_start = (dy as usize + row) * stride_px + dx as usize;
            let src_start = row * src_stride;
            pix[dst_start..dst_start + w as usize]
                .copy_from_slice(&src[src_start..src_start + w as usize]);
        }
    }

    /// Dump the framebuffer to a PPM (P6 binary) file — useful for
    /// eyeballing output on the host where we don't have a real display.
    pub fn dump_ppm(&mut self, path: &str) -> std::io::Result<()> {
        use std::io::Write;
        let w = self.width as usize;
        let h = self.height as usize;
        let stride = self.stride_px;
        let mut out = Vec::with_capacity(20 + w * h * 3);
        let header = format!("P6\n{} {}\n255\n", w, h);
        out.extend_from_slice(header.as_bytes());
        let pix = self.pixels_mut();
        for y in 0..h {
            for x in 0..w {
                let p = pix[y * stride + x];
                out.push((p >> 16) as u8);   // R
                out.push((p >> 8) as u8);    // G
                out.push(p as u8);           // B
            }
        }
        let mut f = std::fs::File::create(path)?;
        f.write_all(&out)?;
        Ok(())
    }
}

impl Drop for Framebuffer {
    fn drop(&mut self) {
        if let Backing::Mmap { ptr, length, .. } = &self.backing {
            unsafe { libc::munmap(*ptr as *mut _, *length); }
        }
    }
}

// Silence `from_raw_fd` unused-import warning when the ioctl path isn't
// compiled for some platform.  We always use it in the fbdev open path.
use std::os::fd::FromRawFd;
