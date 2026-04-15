// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(dead_code)]
//
// Colormap handling — trivial TrueColor.
//
// Our visual is a single TrueColor entry with RGB masks
// 0xFF0000 / 0xFF00 / 0xFF (advertised in `setup::build_success_reply`).
// For TrueColor visuals, colormap allocation is a no-op: the colormap
// isn't actually consulted for pixel lookup.  `AllocColor` just packs
// the requested 16-bit-per-channel RGB into a 32-bit pixel value, and
// rounds the reported "actual" color down to 8-bit precision since
// that's all the hardware supports.
//
// `AllocNamedColor` takes a color name string like "red" or "light grey"
// and resolves it from a small built-in table of the most-used names.
// Real X servers shell out to `rgb.txt`; we hard-code the colors we
// actually encounter in xterm/twm config files.

/// Pack a (r16, g16, b16) triple into an `0x00RRGGBB` pixel value.
/// The inputs are 16-bit (0..=0xFFFF) X11 convention; we discard the
/// low byte to get 8-bit-per-channel.
pub fn pack_pixel(r16: u16, g16: u16, b16: u16) -> u32 {
    let r = (r16 >> 8) as u32;
    let g = (g16 >> 8) as u32;
    let b = (b16 >> 8) as u32;
    (r << 16) | (g << 8) | b
}

/// Rounded-down "actual" channel value that the server can report back
/// to the client: the client asked for `v16` but the hardware can only
/// represent 256 levels per channel.
pub fn round_channel_16(v16: u16) -> u16 {
    (v16 >> 8) << 8
}

/// Look up a color name and return `(r16, g16, b16)`.  Matching is
/// case-insensitive and strips leading/trailing whitespace; hex forms
/// like `#RRGGBB` and `#RRRRGGGGBBBB` are also accepted.
pub fn lookup_named(name: &str) -> Option<(u16, u16, u16)> {
    let trimmed = name.trim();
    if let Some(rest) = trimmed.strip_prefix('#') {
        return parse_hex(rest);
    }
    let lower = trimmed.to_ascii_lowercase();
    let normalized: String = lower.chars().filter(|c| !c.is_whitespace()).collect();
    for (n, r, g, b) in NAMED_COLORS {
        if *n == normalized {
            return Some((expand_byte(*r), expand_byte(*g), expand_byte(*b)));
        }
    }
    None
}

fn expand_byte(v: u8) -> u16 {
    let v = v as u16;
    (v << 8) | v
}

fn parse_hex(s: &str) -> Option<(u16, u16, u16)> {
    match s.len() {
        6 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            Some((expand_byte(r), expand_byte(g), expand_byte(b)))
        }
        12 => {
            let r = u16::from_str_radix(&s[0..4], 16).ok()?;
            let g = u16::from_str_radix(&s[4..8], 16).ok()?;
            let b = u16::from_str_radix(&s[8..12], 16).ok()?;
            Some((r, g, b))
        }
        _ => None,
    }
}

/// A tiny subset of the X11 rgb.txt palette.  Names are matched
/// against input with whitespace removed and lowercased, so both
/// "light grey" and "LightGrey" resolve here.
const NAMED_COLORS: &[(&str, u8, u8, u8)] = &[
    ("black",       0x00, 0x00, 0x00),
    ("white",       0xFF, 0xFF, 0xFF),
    ("red",         0xFF, 0x00, 0x00),
    ("green",       0x00, 0xFF, 0x00),
    ("blue",        0x00, 0x00, 0xFF),
    ("yellow",      0xFF, 0xFF, 0x00),
    ("cyan",        0x00, 0xFF, 0xFF),
    ("magenta",     0xFF, 0x00, 0xFF),
    ("grey",        0xBE, 0xBE, 0xBE),
    ("gray",        0xBE, 0xBE, 0xBE),
    ("lightgrey",   0xD3, 0xD3, 0xD3),
    ("lightgray",   0xD3, 0xD3, 0xD3),
    ("darkgrey",    0xA9, 0xA9, 0xA9),
    ("darkgray",    0xA9, 0xA9, 0xA9),
    ("orange",      0xFF, 0xA5, 0x00),
    ("purple",      0xA0, 0x20, 0xF0),
    ("brown",       0xA5, 0x2A, 0x2A),
    ("pink",        0xFF, 0xC0, 0xCB),
    ("tan",         0xD2, 0xB4, 0x8C),
    ("navy",        0x00, 0x00, 0x80),
    ("maroon",      0x80, 0x00, 0x00),
    ("olive",       0x80, 0x80, 0x00),
    ("teal",        0x00, 0x80, 0x80),
    ("silver",      0xC0, 0xC0, 0xC0),
    ("gold",        0xFF, 0xD7, 0x00),
    ("steelblue",   0x46, 0x82, 0xB4),
    ("lightblue",   0xAD, 0xD8, 0xE6),
    ("royalblue",   0x41, 0x69, 0xE1),
    ("forestgreen", 0x22, 0x8B, 0x22),
    ("limegreen",   0x32, 0xCD, 0x32),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_red() {
        assert_eq!(pack_pixel(0xFFFF, 0, 0), 0x00FF0000);
    }

    #[test]
    fn pack_green() {
        assert_eq!(pack_pixel(0, 0xFFFF, 0), 0x0000FF00);
    }

    #[test]
    fn pack_blue() {
        assert_eq!(pack_pixel(0, 0, 0xFFFF), 0x000000FF);
    }

    #[test]
    fn named_red() {
        assert_eq!(lookup_named("red"), Some((0xFFFF, 0x0000, 0x0000)));
    }

    #[test]
    fn named_case_insensitive() {
        assert_eq!(lookup_named("SteelBlue"), lookup_named("steelblue"));
        assert_eq!(lookup_named("light grey"), lookup_named("LightGrey"));
    }

    #[test]
    fn hex_six() {
        assert_eq!(lookup_named("#FF0080"), Some((0xFFFF, 0x0000, 0x8080)));
    }

    #[test]
    fn hex_twelve() {
        assert_eq!(lookup_named("#FFFF00008080"), Some((0xFFFF, 0x0000, 0x8080)));
    }

    #[test]
    fn unknown_name() {
        assert_eq!(lookup_named("definitelynotacolor"), None);
    }
}
