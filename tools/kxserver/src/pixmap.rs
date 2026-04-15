// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(dead_code)]
//
// Off-screen drawable.
//
// A Pixmap is a rectangular pixel buffer owned by the server on behalf
// of a client.  Clients use them as scratch surfaces: build up a frame
// or an icon in a pixmap, then CopyArea it onto a window.  Unlike
// windows, pixmaps have no tree, no origin-on-screen, and no events —
// they are just a `Vec<u32>` plus metadata.
//
// Phase 5 uses the same 32-bit 0x00RRGGBB layout the framebuffer uses,
// so a CopyArea between a depth-24 pixmap and a window is a byte copy.
// Depth 1 (bitmap) pixmaps are not yet supported — CreatePixmap with
// depth!=24 returns BadMatch.

#[derive(Debug)]
pub struct Pixmap {
    pub width:  u16,
    pub height: u16,
    pub depth:  u8,
    pub pixels: Vec<u32>,
}

impl Pixmap {
    pub fn new(width: u16, height: u16, depth: u8) -> Self {
        let len = width as usize * height as usize;
        Pixmap {
            width,
            height,
            depth,
            pixels: vec![0u32; len],
        }
    }

    /// Stride in pixels — pixmaps are tightly packed.
    pub fn stride_px(&self) -> usize { self.width as usize }

    /// Read a single pixel, returning 0 if out of bounds.
    pub fn get(&self, x: i32, y: i32) -> u32 {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return 0;
        }
        self.pixels[y as usize * self.width as usize + x as usize]
    }

    /// Write a single pixel, ignoring out-of-bounds writes.
    pub fn put(&mut self, x: i32, y: i32, pixel: u32) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        self.pixels[y as usize * self.width as usize + x as usize] = pixel;
    }
}
