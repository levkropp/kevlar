// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(dead_code)]
//
// X11 connection setup.
//
// The client opens the socket and sends a handshake:
//
//     byte  0    byte order: 'l' = 0x6C little-endian, 'B' = 0x42 big-endian
//     byte  1    unused
//     word  2    protocol major version (11)
//     word  4    protocol minor version (0)
//     word  6    authorization name length (n)
//     word  8    authorization data length (d)
//     word 10    unused
//    bytes 12..  authorization name (n bytes) + pad to 4
//    bytes ..    authorization data (d bytes) + pad to 4
//
// The server replies with either:
//     Failed (0): 1 byte tag, 1 byte reason length, 2 bytes major, 2 bytes
//                 minor, 2 bytes length-in-words, reason string + pad
//     Authenticate (2): similar shape, an authentication challenge
//     Success (1): the big block with screens/visuals/formats
//
// We unconditionally return Success and ignore any auth data the client
// presents.  This is strictly more permissive than real Xorg; it works
// because xterm/twm just pass through whatever cookie xauth supplies,
// and we don't care.

use crate::wire::{put_pad, put_u16, put_u32, put_u8};

// ── Setup request (from client) ──────────────────────────────────────

#[derive(Debug)]
pub struct SetupRequest {
    pub byte_order_le: bool,
    pub major: u16,
    pub minor: u16,
    pub auth_name: Vec<u8>,
    pub auth_data: Vec<u8>,
}

#[derive(Debug)]
pub enum SetupError {
    ShortRead,
    BigEndian,
    BadProtocolVersion(u16, u16),
}

pub const SETUP_FIXED_LEN: usize = 12;

/// Parse a client's SetupRequest from a raw byte buffer.  Returns the parsed
/// struct and the number of bytes consumed (which may be larger than the
/// input if we need more data; in that case we return ShortRead).
pub fn parse_setup_request(buf: &[u8]) -> Result<(SetupRequest, usize), SetupError> {
    if buf.len() < SETUP_FIXED_LEN { return Err(SetupError::ShortRead); }

    let byte_order = buf[0];
    if byte_order == 0x42 { return Err(SetupError::BigEndian); }
    if byte_order != 0x6C {
        // Clients that send some other value are definitely broken.  Treat as
        // big-endian refusal; the Server will close.
        return Err(SetupError::BigEndian);
    }

    let major = u16::from_le_bytes([buf[2], buf[3]]);
    let minor = u16::from_le_bytes([buf[4], buf[5]]);
    if major != 11 {
        return Err(SetupError::BadProtocolVersion(major, minor));
    }

    let n = u16::from_le_bytes([buf[6], buf[7]]) as usize;
    let d = u16::from_le_bytes([buf[8], buf[9]]) as usize;
    // buf[10..12] is 2 bytes of pad.

    let n_padded = (n + 3) & !3;
    let d_padded = (d + 3) & !3;
    let total = SETUP_FIXED_LEN + n_padded + d_padded;
    if buf.len() < total { return Err(SetupError::ShortRead); }

    let auth_name = buf[SETUP_FIXED_LEN..SETUP_FIXED_LEN + n].to_vec();
    let auth_data_start = SETUP_FIXED_LEN + n_padded;
    let auth_data = buf[auth_data_start..auth_data_start + d].to_vec();

    Ok((
        SetupRequest {
            byte_order_le: true,
            major,
            minor,
            auth_name,
            auth_data,
        },
        total,
    ))
}

// ── Success reply ────────────────────────────────────────────────────
//
// Shape (per spec):
//
//     byte  0    1 (Success)
//     byte  1    unused
//     word  2    protocol-major-version (11)
//     word  4    protocol-minor-version (0)
//     word  6    length-in-4-byte-units-after-this-header (2+..vendor+formats+screens)
//     dword 8    release number
//     dword 12   resource-id-base
//     dword 16   resource-id-mask
//     dword 20   motion-buffer-size
//     word  24   length of vendor
//     word  26   maximum request length
//     byte  28   number of SCREENs
//     byte  29   number of FORMATs in pixmap-formats
//     byte  30   image-byte-order (0 = LSBFirst, 1 = MSBFirst)
//     byte  31   bitmap-format-bit-order
//     byte  32   bitmap-format-scanline-unit
//     byte  33   bitmap-format-scanline-pad
//     byte  34   min-keycode
//     byte  35   max-keycode
//     dword 36   unused
//     bytes 40   vendor + pad
//     bytes .    LISTofFORMAT (8 bytes each)
//     bytes .    LISTofSCREEN
//
// The "length in 4-byte units" word at offset 6 counts EVERYTHING after
// the 8-byte header up through the end of the reply.  (8 + 4*length) =
// total bytes.

pub const VENDOR: &[u8] = b"Kevlar kxserver";

// Resource IDs we hand out to each client.  The client is told a
// resource-id-base (high bits identifying the client) and a
// resource-id-mask (which bits within an id they can vary).  We give
// each client 2^21 = 2M ids by setting mask = 0x1FFFFF and base =
// (client_id << 21).
pub const RESOURCE_ID_MASK: u32 = 0x001F_FFFF;

pub fn resource_id_base(client_id: u32) -> u32 {
    client_id << 21
}

// Screen geometry constants.  We hard-code a single 1024x768 screen
// that matches the Bochs VGA framebuffer in the default Kevlar boot.
// Later phases will read `FBIOGET_VSCREENINFO` instead.

pub const SCREEN_WIDTH: u16  = 1024;
pub const SCREEN_HEIGHT: u16 = 768;
pub const SCREEN_WIDTH_MM: u16  = 340;   // ~96 DPI
pub const SCREEN_HEIGHT_MM: u16 = 255;

// Fixed resource ids for the one screen we advertise.
pub const ROOT_WINDOW_ID:    u32 = 0x0000_0020;
pub const ROOT_VISUAL_ID:    u32 = 0x0000_0021;
pub const DEFAULT_COLORMAP:  u32 = 0x0000_0022;

/// Build a Success reply to a SetupRequest from the given client.
pub fn build_success_reply(client_id: u32, _req: &SetupRequest) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(256);

    // ── Fixed header (40 bytes) ──
    put_u8(&mut out, 1);          // Success
    put_u8(&mut out, 0);          // unused
    put_u16(&mut out, 11);        // protocol-major-version
    put_u16(&mut out, 0);         // protocol-minor-version

    // length in 4-byte units (we fix this up at the end)
    let length_offset = out.len();
    put_u16(&mut out, 0);

    put_u32(&mut out, 11_00_00_00);      // release number
    put_u32(&mut out, resource_id_base(client_id));
    put_u32(&mut out, RESOURCE_ID_MASK);
    put_u32(&mut out, 0);                // motion-buffer-size
    put_u16(&mut out, VENDOR.len() as u16);
    put_u16(&mut out, 0xFFFF);           // maximum-request-length (words)
    put_u8(&mut out, 1);                 // number of SCREENs
    put_u8(&mut out, 2);                 // number of FORMATs
    put_u8(&mut out, 0);                 // image-byte-order = LSBFirst
    put_u8(&mut out, 0);                 // bitmap-format-bit-order = LeastSig
    put_u8(&mut out, 32);                // bitmap-format-scanline-unit
    put_u8(&mut out, 32);                // bitmap-format-scanline-pad
    put_u8(&mut out, 8);                 // min-keycode
    put_u8(&mut out, 255);               // max-keycode
    put_u32(&mut out, 0);                // unused (pad to 40)
    debug_assert_eq!(out.len(), 40);

    // ── Vendor string ──
    out.extend_from_slice(VENDOR);
    while out.len() % 4 != 0 { out.push(0); }

    // ── FORMATs (8 bytes each, 2 of them) ──
    // Format 1: depth 1, bpp 1
    put_u8(&mut out, 1);   put_u8(&mut out, 1);   put_u8(&mut out, 32);
    put_pad(&mut out, 5);
    // Format 2: depth 24, bpp 32
    put_u8(&mut out, 24);  put_u8(&mut out, 32);  put_u8(&mut out, 32);
    put_pad(&mut out, 5);

    // ── SCREEN (1 of them) ──
    put_u32(&mut out, ROOT_WINDOW_ID);
    put_u32(&mut out, DEFAULT_COLORMAP);
    put_u32(&mut out, 0x00FFFFFF);       // white pixel
    put_u32(&mut out, 0x00000000);       // black pixel
    put_u32(&mut out, 0);                // current input masks
    put_u16(&mut out, SCREEN_WIDTH);
    put_u16(&mut out, SCREEN_HEIGHT);
    put_u16(&mut out, SCREEN_WIDTH_MM);
    put_u16(&mut out, SCREEN_HEIGHT_MM);
    put_u16(&mut out, 1);                // min installed maps
    put_u16(&mut out, 1);                // max installed maps
    put_u32(&mut out, ROOT_VISUAL_ID);
    put_u8(&mut out, 0);                 // backing-stores = Never
    put_u8(&mut out, 0);                 // save-unders = False
    put_u8(&mut out, 24);                // root-depth
    put_u8(&mut out, 1);                 // number of DEPTHs

    // ── DEPTH inside SCREEN ──
    put_u8(&mut out, 24);                // depth
    put_u8(&mut out, 0);                 // unused
    put_u16(&mut out, 1);                // number of VISUALTYPEs
    put_u32(&mut out, 0);                // unused

    // ── VISUALTYPE inside DEPTH ──
    put_u32(&mut out, ROOT_VISUAL_ID);
    put_u8(&mut out, 4);                 // class = TrueColor
    put_u8(&mut out, 8);                 // bits-per-rgb-value
    put_u16(&mut out, 256);              // colormap entries
    put_u32(&mut out, 0x00FF_0000);      // red mask
    put_u32(&mut out, 0x0000_FF00);      // green mask
    put_u32(&mut out, 0x0000_00FF);      // blue mask
    put_u32(&mut out, 0);                // unused (pad to 24 bytes)

    // Fix up length-in-words.  The length field counts everything after
    // the first 8 bytes.
    let total_len = out.len();
    let length_words = ((total_len - 8) / 4) as u16;
    out[length_offset]     = length_words.to_le_bytes()[0];
    out[length_offset + 1] = length_words.to_le_bytes()[1];

    out
}

/// Build a Failed reply with the given human-readable reason.
pub fn build_failed_reply(major: u16, minor: u16, reason: &str) -> Vec<u8> {
    let reason_bytes = reason.as_bytes();
    let n = reason_bytes.len();
    let padded = (n + 3) & !3;
    let mut out = Vec::with_capacity(8 + padded);
    put_u8(&mut out, 0);                // Failed
    put_u8(&mut out, n as u8);
    put_u16(&mut out, major);
    put_u16(&mut out, minor);
    put_u16(&mut out, (padded / 4) as u16);
    out.extend_from_slice(reason_bytes);
    while out.len() % 4 != 0 { out.push(0); }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_length_is_self_consistent() {
        let req = SetupRequest {
            byte_order_le: true,
            major: 11, minor: 0,
            auth_name: Vec::new(), auth_data: Vec::new(),
        };
        let reply = build_success_reply(1, &req);
        assert_eq!(reply[0], 1, "tag is Success");
        assert_eq!(reply.len() % 4, 0, "total length is word-aligned");
        let length_words = u16::from_le_bytes([reply[6], reply[7]]) as usize;
        assert_eq!(8 + 4 * length_words, reply.len(),
                   "length field matches physical size");
        assert!(reply.len() > 40, "has vendor + formats + screen");
    }

    #[test]
    fn parse_minimal_setup_request() {
        // 0x6C = little-endian, major 11, minor 0, zero auth.
        let mut raw = vec![
            0x6C, 0x00,
            11, 0,      // major = 11
            0, 0,       // minor = 0
            0, 0,       // auth name length = 0
            0, 0,       // auth data length = 0
            0, 0,       // pad
        ];
        // pad to 12 bytes
        assert_eq!(raw.len(), 12);
        raw.extend_from_slice(&[]);  // no auth data
        let (req, used) = parse_setup_request(&raw).unwrap();
        assert_eq!(used, 12);
        assert_eq!(req.major, 11);
        assert!(req.auth_name.is_empty());
        assert!(req.auth_data.is_empty());
    }

    #[test]
    fn parse_rejects_big_endian() {
        let raw = [0x42u8, 0, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert!(matches!(parse_setup_request(&raw), Err(SetupError::BigEndian)));
    }
}
