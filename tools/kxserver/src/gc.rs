// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(dead_code)]
//
// Graphics Context.
//
// A GC holds the state that drawing operations consult: foreground
// pixel, background pixel, line width, fill style, function, clip
// rectangles, and so on.  Clients create GCs via `CreateGC`, modify
// them via `ChangeGC`, and pass them as an argument to every drawing
// request (`PolyFillRectangle`, `PolyRectangle`, `ClearArea` — the last
// one doesn't technically need a GC but the others do).
//
// We implement the Phase-4-relevant subset: foreground, background,
// function (only `GXcopy` is honored), line_width (clamped to 1 for
// now), plane_mask (honored as an AND before writing pixels),
// subwindow_mode (stub), clip_origin + clip_rects.  Tile, stipple,
// dashes, arc_mode, cap_style, join_style, fill_style — all parsed
// but mostly ignored; `PolyFillArc` etc. are not in Phase 4 anyway.

#[derive(Debug, Clone, Copy)]
pub enum GcFunction {
    Clear      = 0,
    And        = 1,
    AndReverse = 2,
    Copy       = 3,
    AndInverted = 4,
    NoOp       = 5,
    Xor        = 6,
    Or         = 7,
    Nor        = 8,
    Equiv      = 9,
    Invert     = 10,
    OrReverse  = 11,
    CopyInverted = 12,
    OrInverted = 13,
    Nand       = 14,
    Set        = 15,
}

impl GcFunction {
    pub fn from_u32(v: u32) -> Self {
        match v {
            0 => GcFunction::Clear,
            1 => GcFunction::And,
            2 => GcFunction::AndReverse,
            3 => GcFunction::Copy,
            4 => GcFunction::AndInverted,
            5 => GcFunction::NoOp,
            6 => GcFunction::Xor,
            7 => GcFunction::Or,
            8 => GcFunction::Nor,
            9 => GcFunction::Equiv,
            10 => GcFunction::Invert,
            11 => GcFunction::OrReverse,
            12 => GcFunction::CopyInverted,
            13 => GcFunction::OrInverted,
            14 => GcFunction::Nand,
            15 => GcFunction::Set,
            _  => GcFunction::Copy,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Gc {
    /// XID of the drawable this GC was created against.  We record it
    /// for logging / bookkeeping; drawing uses whatever drawable the
    /// specific request names, not this one.
    pub drawable: u32,

    pub function:    GcFunction,
    pub plane_mask:  u32,
    pub foreground:  u32,
    pub background:  u32,
    pub line_width:  u16,
    pub line_style:  u8,
    pub cap_style:   u8,
    pub join_style:  u8,
    pub fill_style:  u8,
    pub fill_rule:   u8,
    pub tile:        u32,       // pixmap id (unused)
    pub stipple:     u32,       // pixmap id (unused)
    pub ts_x_origin: i16,
    pub ts_y_origin: i16,
    pub font:        u32,       // font id
    pub subwindow_mode: u8,
    pub graphics_exposures: bool,
    pub clip_x_origin:  i16,
    pub clip_y_origin:  i16,
    pub clip_mask:      u32,    // pixmap id, or 0 = none
    pub dash_offset:    u16,
    pub dashes:         u8,
    pub arc_mode:       u8,
    /// Runtime-installed clip rectangles (via SetClipRectangles).
    /// Empty = no additional clipping (default).
    pub clip_rects:    Vec<(i16, i16, u16, u16)>,
}

impl Gc {
    pub fn new_default(drawable: u32) -> Self {
        Gc {
            drawable,
            function:   GcFunction::Copy,
            plane_mask: 0xFFFF_FFFF,
            foreground: 0x00000000,
            background: 0x00FFFFFF,
            line_width: 0,  // 0 means "thin line" per spec
            line_style: 0,  // LineSolid
            cap_style:  1,  // CapButt
            join_style: 0,  // JoinMiter
            fill_style: 0,  // FillSolid
            fill_rule:  0,  // EvenOddRule
            tile:       0,
            stipple:    0,
            ts_x_origin: 0,
            ts_y_origin: 0,
            font:       0,
            subwindow_mode: 0, // ClipByChildren
            graphics_exposures: true,
            clip_x_origin: 0,
            clip_y_origin: 0,
            clip_mask:  0,
            dash_offset: 0,
            dashes:     4,
            arc_mode:   1,  // ArcPieSlice
            clip_rects: Vec::new(),
        }
    }
}

/// Apply a `CreateGC` or `ChangeGC` value list to a GC.  Returns the
/// number of bytes consumed.  The bit numbers here are the `GC*`
/// constants from xproto.h (bit 0 = GCFunction, …).
pub fn apply_values(gc: &mut Gc, mask: u32, mut values: &[u8]) -> usize {
    let start = values.len();
    let bits: &[(u32, fn(&mut Gc, u32))] = &[
        (0,  |gc, v| gc.function    = GcFunction::from_u32(v)),
        (1,  |gc, v| gc.plane_mask  = v),
        (2,  |gc, v| gc.foreground  = v),
        (3,  |gc, v| gc.background  = v),
        (4,  |gc, v| gc.line_width  = v as u16),
        (5,  |gc, v| gc.line_style  = v as u8),
        (6,  |gc, v| gc.cap_style   = v as u8),
        (7,  |gc, v| gc.join_style  = v as u8),
        (8,  |gc, v| gc.fill_style  = v as u8),
        (9,  |gc, v| gc.fill_rule   = v as u8),
        (10, |gc, v| gc.tile        = v),
        (11, |gc, v| gc.stipple     = v),
        (12, |gc, v| gc.ts_x_origin = v as i16),
        (13, |gc, v| gc.ts_y_origin = v as i16),
        (14, |gc, v| gc.font        = v),
        (15, |gc, v| gc.subwindow_mode = v as u8),
        (16, |gc, v| gc.graphics_exposures = v != 0),
        (17, |gc, v| gc.clip_x_origin = v as i16),
        (18, |gc, v| gc.clip_y_origin = v as i16),
        (19, |gc, v| gc.clip_mask    = v),
        (20, |gc, v| gc.dash_offset  = v as u16),
        (21, |gc, v| gc.dashes       = v as u8),
        (22, |gc, v| gc.arc_mode     = v as u8),
    ];
    for (bit, apply) in bits {
        if (mask & (1u32 << bit)) == 0 { continue; }
        if values.len() < 4 { break; }
        let v = u32::from_le_bytes([values[0], values[1], values[2], values[3]]);
        values = &values[4..];
        apply(gc, v);
    }
    start - values.len()
}
