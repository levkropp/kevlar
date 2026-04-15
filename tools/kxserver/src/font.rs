// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(dead_code)]
//
// Font module.
//
// kxserver ships exactly one built-in bitmap font.  Every `OpenFont`
// request — regardless of the XLFD name the client asks for — maps
// to this single font.  That is enough to answer `QueryFont`,
// `QueryTextExtents`, `ListFonts`, and the text drawing requests with
// a consistent font surface.
//
// A full-coverage font is an easy follow-up: swap `font_data::FONT_8X16`
// for a 256-glyph table generated at build time from a PSF2 file.
//
// Glyph format:
//   * fixed 8x16 cells
//   * one byte per row, bit 7 = leftmost pixel
//   * `ascent = 13`, `descent = 3`, total height 16
//
// The wire opcodes that consume this module live in `dispatch.rs`.
// `draw_text_window` rasterizes a single byte string onto a window
// clipped via the existing `ClipRect` pipeline.

use crate::fb::Framebuffer;
use crate::font_data::{ASCENT, CHAR_H, CHAR_W, DESCENT, FONT_8X16};
use crate::render::{put_pixel_clipped, ClipRect};

/// Server-owned font reference.  Resources of this kind point at the
/// single static font blob; they exist so we can track which ids have
/// been OpenFont'd and respect `FreeFont`.
#[derive(Debug, Clone)]
pub struct FontRef {
    pub name:     String,
    pub char_w:   u16,
    pub char_h:   u16,
    pub ascent:   i16,
    pub descent:  i16,
    pub first:    u16,
    pub last:     u16,
}

impl FontRef {
    /// Build a FontRef for the single built-in font.  The requested
    /// XLFD name is retained so logs stay informative even though the
    /// glyphs are always the same.
    pub fn embedded(requested_name: &str) -> Self {
        FontRef {
            name:    requested_name.to_string(),
            char_w:  CHAR_W,
            char_h:  CHAR_H,
            ascent:  ASCENT,
            descent: DESCENT,
            first:   0,
            last:    255,
        }
    }
}

/// Rasterize `text` onto `fb` at window-relative (x, y).  X11 text
/// coordinates are the baseline of the leftmost glyph, so we start
/// drawing rows at `y - ascent`.  Pixels that land outside `clip` (the
/// target window's absolute rect) or outside the framebuffer bounds
/// are dropped.
///
/// When `opaque` is true, every cell is first filled with `bg_pixel`
/// and then overlaid with `fg_pixel` on set glyph bits.  When false
/// (PolyText path), only set bits are drawn.
pub fn draw_text_window(
    fb: &mut Framebuffer,
    abs: (i32, i32),
    clip: ClipRect,
    mut x: i16, y: i16,
    text: &[u8],
    fg_pixel: u32,
    bg_pixel: u32,
    opaque: bool,
) -> i16 {
    let cw = CHAR_W as i32;
    let ch = CHAR_H as i32;
    let ascent = ASCENT as i32;

    // Screen-absolute baseline origin.
    let origin_y_rel = y as i32 - ascent;

    for &b in text {
        let glyph = &FONT_8X16[b as usize];
        for row in 0..ch {
            let bits = glyph[row as usize];
            let py = abs.1 + origin_y_rel + row;
            for col in 0..cw {
                let bit = (bits >> (7 - col)) & 1;
                let px = abs.0 + x as i32 + col;
                if opaque {
                    put_pixel_clipped(fb, clip, px, py,
                        if bit != 0 { fg_pixel } else { bg_pixel });
                } else if bit != 0 {
                    put_pixel_clipped(fb, clip, px, py, fg_pixel);
                }
            }
        }
        x = x.saturating_add(CHAR_W as i16);
    }
    x
}

/// Compute the total advance width (in pixels) of `text` in the
/// built-in font.  Needed for `QueryTextExtents`.
pub fn text_width(text: &[u8]) -> i32 {
    text.len() as i32 * CHAR_W as i32
}
