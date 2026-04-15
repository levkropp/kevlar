// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(dead_code)]
//
// Window properties.
//
// Each window carries a list of (name_atom, type_atom, format, data)
// tuples.  Clients write with `ChangeProperty`, read with
// `GetProperty`, list with `ListProperties`, delete with
// `DeleteProperty`.  Properties drive everything from WM_NAME and
// WM_CLASS to _NET_WM_NAME and the ICCCM inter-client communication
// protocol.
//
// `format` is 8, 16, or 32 (bits per unit).  `data` stores the raw
// bytes; length in units = data.len() / (format/8).

/// A single property on a window.
#[derive(Debug, Clone)]
pub struct Property {
    /// Name atom (e.g. 39 = WM_NAME).
    pub name: u32,
    /// Type atom (e.g. 31 = STRING, 6 = CARDINAL, or a dynamic atom
    /// like UTF8_STRING interned by the client).
    pub ty: u32,
    /// Bits per item: 8, 16, or 32.
    pub format: u8,
    /// Raw bytes.  `data.len() * 8 / format` is the item count.
    pub data: Vec<u8>,
}

impl Property {
    pub fn items(&self) -> usize {
        if self.format == 0 { 0 } else { self.data.len() * 8 / self.format as usize }
    }
}

/// ChangeProperty `mode` field.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeMode {
    Replace = 0,
    Prepend = 1,
    Append  = 2,
}

impl ChangeMode {
    pub fn from_u8(v: u8) -> Option<ChangeMode> {
        match v {
            0 => Some(ChangeMode::Replace),
            1 => Some(ChangeMode::Prepend),
            2 => Some(ChangeMode::Append),
            _ => None,
        }
    }
}
