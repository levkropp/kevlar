// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(dead_code)]
//
// Drawing primitives.
//
// Everything eventually flows through `put_pixel`, which clips to the
// framebuffer bounds and writes a single pixel.  Higher-level ops
// (`fill_rect`, `line`, `rectangle_outline`) walk the primitive area
// and call `put_pixel` for each covered pixel.  Clipping is rectangle-
// based: we intersect the drawing region with the window's bounds
// (walked absolutely through the parent chain) and with the
// framebuffer bounds before writing anything.
//
// This module deliberately does not know about windows or resources —
// the dispatcher looks up the window, computes its absolute screen
// origin, builds a clip rect, and calls the functions here.

use crate::fb::Framebuffer;

/// A screen-absolute rectangle used for clipping.
#[derive(Debug, Clone, Copy)]
pub struct ClipRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl ClipRect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self { ClipRect { x, y, w, h } }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    /// Intersect with the framebuffer bounds and return the result.
    /// Returns an empty rect if there's no overlap.
    pub fn intersect_fb(self, fb_w: u16, fb_h: u16) -> Self {
        let x0 = self.x.max(0);
        let y0 = self.y.max(0);
        let x1 = (self.x + self.w).min(fb_w as i32);
        let y1 = (self.y + self.h).min(fb_h as i32);
        ClipRect { x: x0, y: y0, w: (x1 - x0).max(0), h: (y1 - y0).max(0) }
    }

    pub fn is_empty(&self) -> bool { self.w <= 0 || self.h <= 0 }
}

/// Fill a rectangle in window-relative coordinates.  `abs` is the
/// window's absolute screen origin, `clip` is the final screen-absolute
/// rect the window is allowed to paint into.
pub fn fill_rect_window(
    fb: &mut Framebuffer,
    abs: (i32, i32),
    clip: ClipRect,
    x: i16, y: i16, w: u16, h: u16,
    pixel: u32,
) {
    // Window-relative → screen-absolute.
    let sx = abs.0 + x as i32;
    let sy = abs.1 + y as i32;
    let sw = w as i32;
    let sh = h as i32;
    let rect = ClipRect::new(sx, sy, sw, sh);
    let final_rect = intersect(rect, clip).intersect_fb(fb.width, fb.height);
    if final_rect.is_empty() { return; }
    fb.fill_rect(
        final_rect.x as u16,
        final_rect.y as u16,
        final_rect.w as u16,
        final_rect.h as u16,
        pixel,
    );
}

/// Draw a 1-pixel-wide rectangle outline.  `x,y` is the top-left corner
/// in window coordinates; `w,h` is the size.  Per X11 spec, an outline
/// rectangle's bottom-right corner is at `x + w, y + h` (inclusive on
/// both ends, so a `w`-wide outline is actually `w+1` pixels wide).
pub fn rectangle_outline_window(
    fb: &mut Framebuffer,
    abs: (i32, i32),
    clip: ClipRect,
    x: i16, y: i16, w: u16, h: u16,
    pixel: u32,
) {
    let x0 = abs.0 + x as i32;
    let y0 = abs.1 + y as i32;
    let x1 = x0 + w as i32;
    let y1 = y0 + h as i32;
    // Top and bottom edges.
    for xi in x0..=x1 {
        put_pixel_clipped(fb, clip, xi, y0, pixel);
        put_pixel_clipped(fb, clip, xi, y1, pixel);
    }
    // Left and right edges.
    for yi in y0..=y1 {
        put_pixel_clipped(fb, clip, x0, yi, pixel);
        put_pixel_clipped(fb, clip, x1, yi, pixel);
    }
}

/// Draw a line between two window-relative points using Bresenham.
pub fn line_window(
    fb: &mut Framebuffer,
    abs: (i32, i32),
    clip: ClipRect,
    x0: i16, y0: i16, x1: i16, y1: i16,
    pixel: u32,
) {
    let mut x = abs.0 + x0 as i32;
    let mut y = abs.1 + y0 as i32;
    let xe = abs.0 + x1 as i32;
    let ye = abs.1 + y1 as i32;

    let dx = (xe - x).abs();
    let dy = -(ye - y).abs();
    let sx = if x < xe { 1 } else { -1 };
    let sy = if y < ye { 1 } else { -1 };
    let mut err = dx + dy;

    loop {
        put_pixel_clipped(fb, clip, x, y, pixel);
        if x == xe && y == ye { break; }
        let e2 = 2 * err;
        if e2 >= dy { err += dy; x += sx; }
        if e2 <= dx { err += dx; y += sy; }
    }
}

/// Put a single pixel at screen-absolute coords, clipping to `clip`
/// and the framebuffer bounds.
pub fn put_pixel_clipped(fb: &mut Framebuffer, clip: ClipRect, x: i32, y: i32, pixel: u32) {
    if !clip.contains(x, y) { return; }
    if x < 0 || y < 0 || x >= fb.width as i32 || y >= fb.height as i32 { return; }
    fb.put_pixel(x as u16, y as u16, pixel);
}

/// Blit a rectangle of source pixels from one drawable's backing store
/// to another.  For window → window within the same fb, this is a
/// straight memmove; for pixmap → window or window → pixmap the caller
/// has to supply the source buffer.  Phase 4 only implements the
/// fb-to-fb case (used for CopyArea when both source and destination
/// are mapped windows).
pub fn copy_area_fb_to_fb(
    fb: &mut Framebuffer,
    src_abs: (i32, i32),
    dst_abs: (i32, i32),
    dst_clip: ClipRect,
    x_src: i16, y_src: i16,
    x_dst: i16, y_dst: i16,
    w: u16, h: u16,
) {
    // Simple pixel-by-pixel loop — good enough for Phase 4.  We walk
    // in an order that handles overlap if src and dst are on the same
    // framebuffer and the destination is after the source.
    let sx = src_abs.0 + x_src as i32;
    let sy = src_abs.1 + y_src as i32;
    let dx = dst_abs.0 + x_dst as i32;
    let dy = dst_abs.1 + y_dst as i32;
    let forward = dy < sy || (dy == sy && dx < sx);
    if forward {
        for row in 0..h as i32 {
            for col in 0..w as i32 {
                copy_one(fb, dst_clip, sx + col, sy + row, dx + col, dy + row);
            }
        }
    } else {
        for row in (0..h as i32).rev() {
            for col in (0..w as i32).rev() {
                copy_one(fb, dst_clip, sx + col, sy + row, dx + col, dy + row);
            }
        }
    }
}

fn copy_one(fb: &mut Framebuffer, clip: ClipRect, sx: i32, sy: i32, dx: i32, dy: i32) {
    if sx < 0 || sy < 0 || sx >= fb.width as i32 || sy >= fb.height as i32 { return; }
    if !clip.contains(dx, dy) { return; }
    if dx < 0 || dy < 0 || dx >= fb.width as i32 || dy >= fb.height as i32 { return; }
    let stride = fb.stride_px;
    let pix = fb.pixels_mut();
    let p = pix[sy as usize * stride + sx as usize];
    pix[dy as usize * stride + dx as usize] = p;
}

fn intersect(a: ClipRect, b: ClipRect) -> ClipRect {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.w).min(b.x + b.w);
    let y1 = (a.y + a.h).min(b.y + b.h);
    ClipRect { x: x0, y: y0, w: (x1 - x0).max(0), h: (y1 - y0).max(0) }
}
