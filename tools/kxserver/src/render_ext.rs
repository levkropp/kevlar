// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(dead_code)]
//
// RENDER extension support — Phase 10.
//
// The RENDER extension is the single most important X11 extension
// for modern desktops.  Xft (used by xterm, GTK, Qt, …) renders
// antialiased text by:
//
//   1. Loading a TrueType/OpenType font client-side via freetype.
//   2. Uploading per-glyph bitmaps to the server as a "glyph set" of
//      A8 (8-bit alpha) pixmaps.
//   3. Drawing strings by sending `CompositeGlyphs8` — the server
//      uses the glyph set as a mask over a solid-fill source color
//      picture, with the destination being the window the text
//      should land on.  The Porter-Duff "over" operator does the
//      actual alpha blending.
//
// kxserver implements a tiny subset of RENDER that is sufficient to
// pump Xft's hot path:
//
//   QueryVersion        (0)
//   QueryPictFormats    (1)
//   CreatePicture       (4)
//   FreePicture         (7)
//   Composite           (8)           — Src + Over only
//   CreateGlyphSet      (17)
//   FreeGlyphSet        (19)
//   AddGlyphs           (20)
//   FreeGlyphs          (22)
//   CompositeGlyphs8    (23)          — the text hot path
//   FillRectangles      (26)
//   CreateSolidFill     (33)
//
// Advanced requests (trapezoids, triangles, transforms, gradients)
// are parked as follow-up work.  xterm never touches them.
//
// Alpha compositing uses straight (non-premultiplied) alpha
// throughout.  The math is per-channel for R/G/B and then A.
// `over_pixel_masked` is the one hot routine — everything else is
// wire-protocol bookkeeping.

use std::collections::BTreeMap;

// ═════════════════════════════════════════════════════════════════════
// Picture formats
// ═════════════════════════════════════════════════════════════════════
//
// kxserver advertises five picture formats.  The ids are picked
// arbitrarily but fit in the server-owned XID range (below any
// client range).

pub const PICTFORMAT_A8R8G8B8: u32 = 0x0000_0010;
pub const PICTFORMAT_X8R8G8B8: u32 = 0x0000_0011;
pub const PICTFORMAT_R5G6B5:   u32 = 0x0000_0012;
pub const PICTFORMAT_A8:       u32 = 0x0000_0013;
pub const PICTFORMAT_A1:       u32 = 0x0000_0014;

/// Type byte for a picture format.
pub const PICT_TYPE_INDEXED: u8 = 0;
pub const PICT_TYPE_DIRECT:  u8 = 1;

#[derive(Debug, Clone, Copy)]
pub struct PictDirect {
    pub red_shift:   u16,
    pub red_mask:    u16,
    pub green_shift: u16,
    pub green_mask:  u16,
    pub blue_shift:  u16,
    pub blue_mask:   u16,
    pub alpha_shift: u16,
    pub alpha_mask:  u16,
}

#[derive(Debug, Clone, Copy)]
pub struct PictFormatInfo {
    pub id:     u32,
    pub ty:     u8,
    pub depth:  u8,
    pub direct: PictDirect,
    pub colormap: u32,
}

pub const PICT_FORMATS: &[PictFormatInfo] = &[
    // A8R8G8B8 — the main 32-bit format, alpha in bits 24..32.
    PictFormatInfo {
        id: PICTFORMAT_A8R8G8B8, ty: PICT_TYPE_DIRECT, depth: 32,
        direct: PictDirect {
            red_shift: 16, red_mask: 0xFF,
            green_shift: 8, green_mask: 0xFF,
            blue_shift: 0, blue_mask: 0xFF,
            alpha_shift: 24, alpha_mask: 0xFF,
        },
        colormap: 0,
    },
    // X8R8G8B8 — same bit layout minus alpha (for depth-24 windows).
    PictFormatInfo {
        id: PICTFORMAT_X8R8G8B8, ty: PICT_TYPE_DIRECT, depth: 24,
        direct: PictDirect {
            red_shift: 16, red_mask: 0xFF,
            green_shift: 8, green_mask: 0xFF,
            blue_shift: 0, blue_mask: 0xFF,
            alpha_shift: 0, alpha_mask: 0x00,
        },
        colormap: 0,
    },
    // R5G6B5 — 16-bit RGB, occasionally used for mask pixmaps.
    PictFormatInfo {
        id: PICTFORMAT_R5G6B5, ty: PICT_TYPE_DIRECT, depth: 16,
        direct: PictDirect {
            red_shift: 11, red_mask: 0x1F,
            green_shift: 5, green_mask: 0x3F,
            blue_shift: 0, blue_mask: 0x1F,
            alpha_shift: 0, alpha_mask: 0x00,
        },
        colormap: 0,
    },
    // A8 — 8-bit alpha-only, used for antialiased glyph masks.
    PictFormatInfo {
        id: PICTFORMAT_A8, ty: PICT_TYPE_DIRECT, depth: 8,
        direct: PictDirect {
            red_shift: 0, red_mask: 0x00,
            green_shift: 0, green_mask: 0x00,
            blue_shift: 0, blue_mask: 0x00,
            alpha_shift: 0, alpha_mask: 0xFF,
        },
        colormap: 0,
    },
    // A1 — 1-bit bitmap mask, used for classic bitmap glyphs.
    PictFormatInfo {
        id: PICTFORMAT_A1, ty: PICT_TYPE_DIRECT, depth: 1,
        direct: PictDirect {
            red_shift: 0, red_mask: 0x00,
            green_shift: 0, green_mask: 0x00,
            blue_shift: 0, blue_mask: 0x00,
            alpha_shift: 0, alpha_mask: 0x01,
        },
        colormap: 0,
    },
];

/// Check whether a given picture format id is one we advertise.
pub fn pict_format_exists(id: u32) -> bool {
    PICT_FORMATS.iter().any(|f| f.id == id)
}

/// Look up a format by id.
pub fn pict_format(id: u32) -> Option<&'static PictFormatInfo> {
    PICT_FORMATS.iter().find(|f| f.id == id)
}

// ═════════════════════════════════════════════════════════════════════
// Picture resource
// ═════════════════════════════════════════════════════════════════════
//
// A picture wraps either a drawable (window/pixmap) OR a solid fill,
// plus the picture format it should be interpreted in and a handful
// of rendering attributes.

#[derive(Debug, Clone)]
pub struct Picture {
    /// The drawable id (window/pixmap) this picture is a view of,
    /// or 0 if this is a solid-fill picture.
    pub drawable: u32,
    /// The picture format id.
    pub format: u32,
    /// Repeat mode: 0=None, 1=Normal, 2=Pad, 3=Reflect.
    pub repeat: u8,
    /// For solid-fill pictures, the color in A8R8G8B8.  None for
    /// drawable-backed pictures.
    pub solid: Option<u32>,
    /// Clip rectangles (none by default).  Empty vec = unclipped.
    pub clip_rects: Vec<(i16, i16, u16, u16)>,
    /// Optional XFIXES region installed as the clip via
    /// SetPictureClipRegion (Phase 11).  Coordinates are relative to
    /// the picture's destination drawable origin, so compositing
    /// checks it as `region.contains(dst_local_x, dst_local_y)`.
    pub clip_region: Option<crate::region::Region>,
}

impl Picture {
    pub fn new_drawable(drawable: u32, format: u32) -> Self {
        Picture {
            drawable,
            format,
            repeat: 0,
            solid: None,
            clip_rects: Vec::new(),
            clip_region: None,
        }
    }

    pub fn new_solid(color_argb: u32) -> Self {
        Picture {
            drawable: 0,
            format: PICTFORMAT_A8R8G8B8,
            repeat: 1,
            solid: Some(color_argb),
            clip_rects: Vec::new(),
            clip_region: None,
        }
    }
}

// ═════════════════════════════════════════════════════════════════════
// Glyph sets
// ═════════════════════════════════════════════════════════════════════

/// A single uploaded glyph.  Phase 10 supports A8 (grayscale alpha)
/// and A1 (1-bit) storage; the bytes vec is row-major.
#[derive(Debug, Clone)]
pub struct Glyph {
    pub width:  u16,
    pub height: u16,
    /// Origin offset from the cursor position — the glyph's
    /// bearing.  For a left-bearing char like 'j' this is negative.
    pub x:      i16,
    pub y:      i16,
    /// Advance after the glyph is drawn.
    pub x_off:  i16,
    pub y_off:  i16,
    pub data:   Vec<u8>,
}

impl Glyph {
    /// Sample the A8 alpha at (gx, gy).  Out-of-bounds reads return 0.
    /// For A1 glyph sets the caller is responsible for expanding
    /// the bit into 0 or 255 before calling.
    pub fn alpha_at(&self, gx: i32, gy: i32) -> u8 {
        if gx < 0 || gy < 0 || gx >= self.width as i32 || gy >= self.height as i32 {
            return 0;
        }
        self.data[gy as usize * self.width as usize + gx as usize]
    }
}

#[derive(Debug, Clone)]
pub struct GlyphSet {
    pub format: u32,
    pub glyphs: BTreeMap<u32, Glyph>,
}

impl GlyphSet {
    pub fn new(format: u32) -> Self {
        GlyphSet { format, glyphs: BTreeMap::new() }
    }
}

// ═════════════════════════════════════════════════════════════════════
// Compositing operators
// ═════════════════════════════════════════════════════════════════════

pub const PICT_OP_CLEAR:     u8 = 0;
pub const PICT_OP_SRC:       u8 = 1;
pub const PICT_OP_DST:       u8 = 2;
pub const PICT_OP_OVER:      u8 = 3;
pub const PICT_OP_OVER_REV:  u8 = 4;
pub const PICT_OP_IN:        u8 = 5;
pub const PICT_OP_OUT:       u8 = 6;
pub const PICT_OP_ATOP:      u8 = 7;
pub const PICT_OP_XOR:       u8 = 10;
pub const PICT_OP_ADD:       u8 = 12;

/// Straight-alpha Porter-Duff `Over` for one A8R8G8B8 pixel.
/// `src` is the source pixel, `dst` is the destination, both in
/// 0xAARRGGBB.  Returns the composited pixel.
#[inline]
pub fn over_pixel(src: u32, dst: u32) -> u32 {
    let sa = ((src >> 24) & 0xFF) as u32;
    if sa == 0 { return dst; }
    if sa == 0xFF { return src; }
    let sr = (src >> 16) & 0xFF;
    let sg = (src >> 8)  & 0xFF;
    let sb =  src        & 0xFF;
    let da = (dst >> 24) & 0xFF;
    let dr = (dst >> 16) & 0xFF;
    let dg = (dst >> 8)  & 0xFF;
    let db =  dst        & 0xFF;
    let inv_sa = 255 - sa;
    // Per-channel: out = src + dst * (1 - src_alpha), straight alpha.
    let r = sr + div255(dr * inv_sa);
    let g = sg + div255(dg * inv_sa);
    let b = sb + div255(db * inv_sa);
    let a = sa + div255(da * inv_sa);
    (a << 24) | (r << 16) | (g << 8) | b
}

/// Source-coverage composite: dst = (src * mask_alpha) over dst.
/// `mask_alpha` is a u8 from the glyph bitmap (0..=255).
#[inline]
pub fn over_pixel_masked(src: u32, dst: u32, mask_alpha: u8) -> u32 {
    if mask_alpha == 0 { return dst; }
    if mask_alpha == 0xFF { return over_pixel(src, dst); }
    let ma = mask_alpha as u32;
    let sa = ((src >> 24) & 0xFF) as u32;
    let sr = (src >> 16) & 0xFF;
    let sg = (src >> 8)  & 0xFF;
    let sb =  src        & 0xFF;
    // Effective source pixel is src scaled by mask_alpha / 255 (all
    // channels including alpha).
    let esa = div255(sa * ma);
    let esr = div255(sr * ma);
    let esg = div255(sg * ma);
    let esb = div255(sb * ma);
    let effective_src = (esa << 24) | (esr << 16) | (esg << 8) | esb;
    over_pixel(effective_src, dst)
}

/// Exact integer `v / 255`.  We keep this as a dedicated inline so
/// the hot compositor reads clearly; the compiler already turns
/// `v / 255` into a `u32` multiply-shift.
#[inline]
fn div255(v: u32) -> u32 {
    v / 255
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn div255_matches_exact() {
        for v in 0..=(255u32 * 255) {
            assert_eq!(div255(v), v / 255, "v={v}");
        }
    }

    #[test]
    fn over_opaque_source() {
        // Fully opaque red source over anything gives opaque red.
        let red = 0xFFFF0000;
        assert_eq!(over_pixel(red, 0xFF000000), red);
        assert_eq!(over_pixel(red, 0xFF00FF00), red);
    }

    #[test]
    fn over_transparent_source() {
        // Fully transparent source leaves destination unchanged.
        let clear = 0x00FF0000;
        let dst = 0xFF00FF00;
        assert_eq!(over_pixel(clear, dst), dst);
    }

    #[test]
    fn over_masked_zero_mask() {
        let src = 0xFFFF0000;
        let dst = 0xFF0000FF;
        assert_eq!(over_pixel_masked(src, dst, 0), dst);
    }

    #[test]
    fn over_masked_full_mask_opaque() {
        let src = 0xFFFF0000;
        let dst = 0xFF0000FF;
        assert_eq!(over_pixel_masked(src, dst, 255), src);
    }

    #[test]
    fn over_masked_half_mask() {
        // Half-alpha red over black → ~0x7F7F0000
        let src = 0xFFFF0000;
        let dst = 0xFF000000;
        let out = over_pixel_masked(src, dst, 128);
        let out_a = (out >> 24) & 0xFF;
        let out_r = (out >> 16) & 0xFF;
        let out_g = (out >> 8) & 0xFF;
        let out_b =  out       & 0xFF;
        // alpha stays at 0xFF because dst is 0xFF and src blends in.
        assert_eq!(out_a, 0xFF);
        assert!((out_r as i32 - 128).abs() <= 1, "red channel {out_r}");
        assert_eq!(out_g, 0);
        assert_eq!(out_b, 0);
    }
}
