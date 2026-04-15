// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(dead_code)]  // phase-1 scaffolding; consumed in later phases
//
// X11 wire protocol primitives.
//
// This module deliberately avoids any generated-from-XML crate (x11rb,
// xcb-types, etc.) and hand-rolls every byte so that every field is
// inspectable in our own source.  That is the whole point of kxserver.
//
// We only support little-endian clients.  A big-endian client would set
// byte-order = 'B' = 0x42 in the handshake; we detect this and refuse the
// connection.  Real Xorg byte-swaps on the fly; that's an order of magnitude
// more complexity than we need for diagnostic work.

use crate::log;

/// Round `n` up to the next multiple of 4.
pub const fn pad4(n: usize) -> usize {
    (n + 3) & !3
}

/// Number of pad bytes needed to reach a 4-byte boundary after `n` bytes.
pub const fn pad4_rem(n: usize) -> usize {
    pad4(n) - n
}

// ── Little-endian readers ─────────────────────────────────────────────

#[inline] pub fn get_u8(buf: &[u8], off: usize) -> u8 { buf[off] }

#[inline]
pub fn get_u16(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}

#[inline]
pub fn get_u32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

// ── Little-endian writers (append to Vec<u8>) ─────────────────────────

#[inline] pub fn put_u8(out: &mut Vec<u8>, v: u8) { out.push(v); }

#[inline]
pub fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

#[inline]
pub fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub fn put_pad(out: &mut Vec<u8>, n: usize) {
    for _ in 0..n { out.push(0); }
}

pub fn align4(out: &mut Vec<u8>) {
    put_pad(out, pad4_rem(out.len()));
}

// ── Request header (first 4 bytes of every request) ──────────────────
//
//     byte  0  major opcode
//     byte  1  data (varies per request)
//     word  2  request length in 4-byte units (including the header)
//
// Requests shorter than 4 bytes are not legal; every request has at
// least this header.  For most requests, data[1] is the length in
// 4-byte units (including this header); for `BigRequests`-enabled
// clients a zero length means "follow with a 32-bit extended length".
// We do NOT advertise the BigRequests extension.

#[derive(Debug, Clone, Copy)]
pub struct RequestHeader {
    pub opcode: u8,
    pub data: u8,       // aka minor opcode / first payload byte
    pub length_words: u16,
}

impl RequestHeader {
    pub const SIZE: usize = 4;

    pub fn parse(buf: &[u8]) -> Option<Self> {
        if buf.len() < Self::SIZE { return None; }
        Some(RequestHeader {
            opcode: buf[0],
            data:   buf[1],
            length_words: get_u16(buf, 2),
        })
    }

    pub fn length_bytes(&self) -> usize {
        (self.length_words as usize) * 4
    }
}

// ── Error block (32 bytes) ────────────────────────────────────────────
//
// Every protocol error fills a fixed 32-byte block.  Layout:
//
//     0:   0 (Error)
//     1:   error code
//     2:   sequence number (u16)
//     4:   bad resource id (u32, 0 if not applicable)
//     8:   minor opcode (u16)
//    10:   major opcode (u8)
//    11:   21 bytes unused

pub const ERROR_BLOCK_SIZE: usize = 32;

/// X11 standard error codes.
pub mod errcode {
    pub const BAD_REQUEST:     u8 = 1;
    pub const BAD_VALUE:       u8 = 2;
    pub const BAD_WINDOW:      u8 = 3;
    pub const BAD_PIXMAP:      u8 = 4;
    pub const BAD_ATOM:        u8 = 5;
    pub const BAD_CURSOR:      u8 = 6;
    pub const BAD_FONT:        u8 = 7;
    pub const BAD_MATCH:       u8 = 8;
    pub const BAD_DRAWABLE:    u8 = 9;
    pub const BAD_ACCESS:      u8 = 10;
    pub const BAD_ALLOC:       u8 = 11;
    pub const BAD_COLORMAP:    u8 = 12;
    pub const BAD_GCONTEXT:    u8 = 13;
    pub const BAD_IDCHOICE:    u8 = 14;
    pub const BAD_NAME:        u8 = 15;
    pub const BAD_LENGTH:      u8 = 16;
    pub const BAD_IMPLEMENTATION: u8 = 17;
}

pub fn build_error(
    code: u8,
    seq: u16,
    bad_resource: u32,
    major_opcode: u8,
    minor_opcode: u16,
) -> [u8; ERROR_BLOCK_SIZE] {
    let mut out = [0u8; ERROR_BLOCK_SIZE];
    out[0] = 0;                          // Error
    out[1] = code;
    out[2..4].copy_from_slice(&seq.to_le_bytes());
    out[4..8].copy_from_slice(&bad_resource.to_le_bytes());
    out[8..10].copy_from_slice(&minor_opcode.to_le_bytes());
    out[10] = major_opcode;
    // bytes 11..32 are pad (already zero)
    out
}

// ── Reply header (first 8 bytes of every reply) ──────────────────────
//
//     byte  0  1 (Reply)
//     byte  1  varies per reply
//     word  2  sequence number (u16)
//     dword 4  reply length in 4-byte units (additional length beyond
//              the 32-byte minimum).  A reply with length=0 has 32 bytes
//              total, length=1 has 32+4 = 36, etc.
//
// Every reply is at least 32 bytes.  Replies longer than 32 bytes set
// the length field to (total_bytes - 32) / 4.

pub const REPLY_MIN_SIZE: usize = 32;

pub fn put_reply_header(out: &mut Vec<u8>, seq: u16, byte1: u8, extra_words: u32) {
    put_u8(out, 1);              // Reply
    put_u8(out, byte1);
    put_u16(out, seq);
    put_u32(out, extra_words);
}

// ── Event block (32 bytes) ────────────────────────────────────────────
//
// Events are always exactly 32 bytes.  Byte 0 is the event code; the
// high bit (0x80) is set if this is a SendEvent synthetic.  Bytes 2..4
// carry the sequence number of the *last request processed*.

pub const EVENT_BLOCK_SIZE: usize = 32;

pub fn new_event(code: u8) -> [u8; EVENT_BLOCK_SIZE] {
    let mut ev = [0u8; EVENT_BLOCK_SIZE];
    ev[0] = code;
    ev
}

// ── Request name lookup for logs ──────────────────────────────────────
//
// Just the names; no schema.  Every later phase will add semantics.

pub fn opcode_name(op: u8) -> &'static str {
    match op {
        1   => "CreateWindow",
        2   => "ChangeWindowAttributes",
        3   => "GetWindowAttributes",
        4   => "DestroyWindow",
        5   => "DestroySubwindows",
        6   => "ChangeSaveSet",
        7   => "ReparentWindow",
        8   => "MapWindow",
        9   => "MapSubwindows",
        10  => "UnmapWindow",
        11  => "UnmapSubwindows",
        12  => "ConfigureWindow",
        13  => "CirculateWindow",
        14  => "GetGeometry",
        15  => "QueryTree",
        16  => "InternAtom",
        17  => "GetAtomName",
        18  => "ChangeProperty",
        19  => "DeleteProperty",
        20  => "GetProperty",
        21  => "ListProperties",
        22  => "SetSelectionOwner",
        23  => "GetSelectionOwner",
        24  => "ConvertSelection",
        25  => "SendEvent",
        26  => "GrabPointer",
        27  => "UngrabPointer",
        28  => "GrabButton",
        29  => "UngrabButton",
        30  => "ChangeActivePointerGrab",
        31  => "GrabKeyboard",
        32  => "UngrabKeyboard",
        33  => "GrabKey",
        34  => "UngrabKey",
        35  => "AllowEvents",
        36  => "GrabServer",
        37  => "UngrabServer",
        38  => "QueryPointer",
        39  => "GetMotionEvents",
        40  => "TranslateCoordinates",
        41  => "WarpPointer",
        42  => "SetInputFocus",
        43  => "GetInputFocus",
        44  => "QueryKeymap",
        45  => "OpenFont",
        46  => "CloseFont",
        47  => "QueryFont",
        48  => "QueryTextExtents",
        49  => "ListFonts",
        50  => "ListFontsWithInfo",
        51  => "SetFontPath",
        52  => "GetFontPath",
        53  => "CreatePixmap",
        54  => "FreePixmap",
        55  => "CreateGC",
        56  => "ChangeGC",
        57  => "CopyGC",
        58  => "SetDashes",
        59  => "SetClipRectangles",
        60  => "FreeGC",
        61  => "ClearArea",
        62  => "CopyArea",
        63  => "CopyPlane",
        64  => "PolyPoint",
        65  => "PolyLine",
        66  => "PolySegment",
        67  => "PolyRectangle",
        68  => "PolyArc",
        69  => "FillPoly",
        70  => "PolyFillRectangle",
        71  => "PolyFillArc",
        72  => "PutImage",
        73  => "GetImage",
        74  => "PolyText8",
        75  => "PolyText16",
        76  => "ImageText8",
        77  => "ImageText16",
        78  => "CreateColormap",
        79  => "FreeColormap",
        80  => "CopyColormapAndFree",
        81  => "InstallColormap",
        82  => "UninstallColormap",
        83  => "ListInstalledColormaps",
        84  => "AllocColor",
        85  => "AllocNamedColor",
        86  => "AllocColorCells",
        87  => "AllocColorPlanes",
        88  => "FreeColors",
        89  => "StoreColors",
        90  => "StoreNamedColor",
        91  => "QueryColors",
        92  => "LookupColor",
        93  => "CreateCursor",
        94  => "CreateGlyphCursor",
        95  => "FreeCursor",
        96  => "RecolorCursor",
        97  => "QueryBestSize",
        98  => "QueryExtension",
        99  => "ListExtensions",
        100 => "ChangeKeyboardMapping",
        101 => "GetKeyboardMapping",
        102 => "ChangeKeyboardControl",
        103 => "GetKeyboardControl",
        104 => "Bell",
        105 => "ChangePointerControl",
        106 => "GetPointerControl",
        107 => "SetScreenSaver",
        108 => "GetScreenSaver",
        109 => "ChangeHosts",
        110 => "ListHosts",
        111 => "SetAccessControl",
        112 => "SetCloseDownMode",
        113 => "KillClient",
        114 => "RotateProperties",
        115 => "ForceScreenSaver",
        116 => "SetPointerMapping",
        117 => "GetPointerMapping",
        118 => "SetModifierMapping",
        119 => "GetModifierMapping",
        127 => "NoOperation",
        128 => "BigReqEnable",
        129 => "RENDER",
        130 => "XFIXES",
        _   => "Unknown",
    }
}

/// True if the opcode expects a reply.  Drives our "unhandled request"
/// behavior: opcodes that expect replies must get either a reply or an
/// error — returning nothing would hang the client.
pub fn opcode_expects_reply(op: u8) -> bool {
    matches!(op,
        3 | 14 | 15 | 16 | 17 | 20 | 21 | 23 | 26 | 31 | 38 |
        39 | 40 | 43 | 44 | 47 | 48 | 49 | 50 | 52 | 73 |
        83 | 84 | 85 | 86 | 87 | 91 | 92 | 97 | 98 | 99 |
        101 | 103 | 106 | 108 | 110 | 116 | 117 | 119 |
        // BIG-REQUESTS:BigReqEnable (major 128, minor 0)
        128 |
        // RENDER and XFIXES: dispatched via their own multiplexers,
        // reply logic is per minor opcode.  Listed here so the
        // unhandled-opcode fallback never synthesizes BadImplementation.
        129 | 130
    )
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pad4() {
        assert_eq!(pad4(0), 0);
        assert_eq!(pad4(1), 4);
        assert_eq!(pad4(4), 4);
        assert_eq!(pad4(5), 8);
        assert_eq!(pad4_rem(0), 0);
        assert_eq!(pad4_rem(1), 3);
        assert_eq!(pad4_rem(4), 0);
    }

    #[test]
    fn test_put_get() {
        let mut buf = Vec::new();
        put_u8(&mut buf, 0x12);
        put_u16(&mut buf, 0x3456);
        put_u32(&mut buf, 0x789ABCDE);
        assert_eq!(buf, vec![0x12, 0x56, 0x34, 0xDE, 0xBC, 0x9A, 0x78]);
        assert_eq!(get_u8(&buf, 0), 0x12);
        assert_eq!(get_u16(&buf, 1), 0x3456);
        assert_eq!(get_u32(&buf, 3), 0x789ABCDE);
    }

    #[test]
    fn test_request_header() {
        let raw = [55u8, 0, 4, 0];  // CreateGC, length 4 words
        let h = RequestHeader::parse(&raw).unwrap();
        assert_eq!(h.opcode, 55);
        assert_eq!(h.length_words, 4);
        assert_eq!(h.length_bytes(), 16);
    }

    #[test]
    fn test_error_block() {
        let e = build_error(errcode::BAD_IMPLEMENTATION, 42, 0, 55, 0);
        assert_eq!(e[0], 0);
        assert_eq!(e[1], 17);
        assert_eq!(u16::from_le_bytes([e[2], e[3]]), 42);
        assert_eq!(e[10], 55);
        assert_eq!(e.len(), 32);
    }
}

// Silence unused-import when built without tests.
#[allow(unused_imports)]
use log as _;
