// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Printf-class formatter for `printk`.  Walks the C format string
//! byte-by-byte and consumes args from a `VaListImpl` for each
//! conversion.  Supports a subset sufficient for any standard
//! Linux module's `printk("...", ...)`:
//!
//!   `%d` `%i`            — signed decimal
//!   `%u`                 — unsigned decimal
//!   `%x` `%X`            — hex (lower / upper)
//!   `%p`                 — pointer (lowercase hex with `0x` prefix)
//!   `%s`                 — NUL-terminated string
//!   `%c`                 — single character
//!   `%%`                 — literal '%'
//!
//! Flags: `0` (zero-pad).  Width: `<digits>`.  Length modifiers
//! `l`, `ll`, `z`, `h`, `hh` are parsed and honored for sign-
//! /zero-extension width; `j` and `t` are accepted but treated
//! as `ll`.
//!
//! Exotic Linux pointer modifiers (`%pK`, `%pf`, `%pe`, …),
//! floating-point, `%n`, locale wide-char are all skipped.

use core::ffi::{c_char, VaList};

const MAX_NUMBUF: usize = 32;

/// Output sink: append bytes until full; further writes drop.
pub struct Sink<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> Sink<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    pub fn pos(&self) -> usize {
        self.pos
    }
    pub fn as_slice(&self) -> &[u8] {
        &self.buf[..self.pos]
    }
    fn push(&mut self, b: u8) {
        if self.pos < self.buf.len() {
            self.buf[self.pos] = b;
            self.pos += 1;
        }
    }
    fn push_bytes(&mut self, src: &[u8]) {
        for &b in src {
            self.push(b);
        }
    }
}

#[derive(Default, Clone, Copy)]
struct Spec {
    zero_pad: bool,
    width: usize,
    has_precision: bool,
    precision: usize,
    length: LenMod,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum LenMod {
    #[default]
    Default,
    Hh,
    H,
    L,
    Ll,
    Z,
}

/// Parse + format from a C format string.  Returns the number of
/// bytes appended to `sink`.  `args` is consumed in step.
///
/// `fmt_ptr` must be a valid NUL-terminated C string.
pub unsafe fn format_into(
    sink: &mut Sink<'_>,
    fmt_ptr: *const c_char,
    args: &mut VaList<'_>,
) -> usize {
    if fmt_ptr.is_null() {
        return 0;
    }

    // Strip Linux KERN_INFO/KERN_ERR/etc preamble: 3 bytes,
    // \x01 + level digit + maybe \n at start of fmt.  Strip up
    // to 4 leading bytes if first byte is SOH.
    let mut p = fmt_ptr as *const u8;
    let mut bytes_consumed = 0usize;
    let first = *p;
    if first == 0x01 {
        p = p.add(1);
        bytes_consumed = 1;
        let lvl = *p;
        if lvl >= b'0' && lvl <= b'7' {
            p = p.add(1);
            bytes_consumed += 1;
        }
        // Some kernel headers also emit \x01 'd' (for KERN_DEFAULT)
        // or other letters; just advance one extra non-NUL byte.
    }
    let _ = bytes_consumed;

    let start = sink.pos();

    // Bound the parse to avoid runaway on malformed input.
    let mut idx = 0usize;
    while idx < 4096 {
        let c = *p;
        if c == 0 {
            break;
        }
        if c != b'%' {
            sink.push(c);
            p = p.add(1);
            idx += 1;
            continue;
        }

        // ── Format spec parser ───────────────────────────────
        p = p.add(1);
        idx += 1;
        let mut spec = Spec::default();

        // Flags (zero or more).
        loop {
            let f = *p;
            match f {
                b'0' => spec.zero_pad = true,
                b'-' | b'+' | b' ' | b'#' => {} // accept and ignore
                _ => break,
            }
            p = p.add(1);
            idx += 1;
        }

        // Width (decimal digits).
        while *p >= b'0' && *p <= b'9' {
            spec.width = spec.width.saturating_mul(10) + (*p - b'0') as usize;
            p = p.add(1);
            idx += 1;
        }

        // Precision: '.<digits>'
        if *p == b'.' {
            spec.has_precision = true;
            p = p.add(1);
            idx += 1;
            while *p >= b'0' && *p <= b'9' {
                spec.precision = spec.precision.saturating_mul(10) + (*p - b'0') as usize;
                p = p.add(1);
                idx += 1;
            }
        }

        // Length modifier.
        match *p {
            b'h' => {
                p = p.add(1);
                idx += 1;
                if *p == b'h' {
                    p = p.add(1);
                    idx += 1;
                    spec.length = LenMod::Hh;
                } else {
                    spec.length = LenMod::H;
                }
            }
            b'l' => {
                p = p.add(1);
                idx += 1;
                if *p == b'l' {
                    p = p.add(1);
                    idx += 1;
                    spec.length = LenMod::Ll;
                } else {
                    spec.length = LenMod::L;
                }
            }
            b'z' | b'j' | b't' => {
                p = p.add(1);
                idx += 1;
                spec.length = LenMod::Z;
            }
            _ => {}
        }

        // Conversion.
        let conv = *p;
        p = p.add(1);
        idx += 1;

        match conv {
            b'%' => sink.push(b'%'),
            b'c' => {
                let v: i32 = unsafe { args.arg::<i32>() };
                sink.push(v as u8);
            }
            b's' => {
                let s: *const c_char = unsafe { args.arg::<*const c_char>() };
                emit_str(sink, s, &spec);
            }
            b'd' | b'i' => {
                let v: i64 = unsafe { read_signed(&spec, args) };
                emit_signed(sink, v, &spec);
            }
            b'u' => {
                let v: u64 = unsafe { read_unsigned(&spec, args) };
                emit_unsigned_dec(sink, v, &spec);
            }
            b'x' => {
                let v: u64 = unsafe { read_unsigned(&spec, args) };
                emit_hex(sink, v, &spec, false, false);
            }
            b'X' => {
                let v: u64 = unsafe { read_unsigned(&spec, args) };
                emit_hex(sink, v, &spec, true, false);
            }
            b'p' => {
                let v: usize = unsafe { args.arg::<usize>() };
                // Optional `%p<modifier>` — skip any letter that
                // immediately follows `p` and use plain hex with
                // 0x prefix.
                while *p != 0 && (*p).is_ascii_alphabetic() {
                    p = p.add(1);
                    idx += 1;
                }
                emit_hex(sink, v as u64, &spec, false, true);
            }
            0 => break,
            _ => {
                // Unknown conversion — drop %, the conversion char.
                sink.push(b'%');
                sink.push(conv);
            }
        }
    }

    sink.pos() - start
}

unsafe fn read_signed(spec: &Spec, args: &mut VaList<'_>) -> i64 {
    unsafe {
        match spec.length {
            LenMod::Hh => args.arg::<i32>() as i8 as i64,
            LenMod::H => args.arg::<i32>() as i16 as i64,
            LenMod::L | LenMod::Ll | LenMod::Z => args.arg::<i64>(),
            LenMod::Default => args.arg::<i32>() as i64,
        }
    }
}

unsafe fn read_unsigned(spec: &Spec, args: &mut VaList<'_>) -> u64 {
    unsafe {
        match spec.length {
            LenMod::Hh => args.arg::<u32>() as u8 as u64,
            LenMod::H => args.arg::<u32>() as u16 as u64,
            LenMod::L | LenMod::Ll | LenMod::Z => args.arg::<u64>(),
            LenMod::Default => args.arg::<u32>() as u64,
        }
    }
}

fn emit_str(sink: &mut Sink<'_>, s: *const c_char, spec: &Spec) {
    if s.is_null() {
        let placeholder = b"(null)";
        let n = if spec.has_precision { spec.precision.min(placeholder.len()) } else { placeholder.len() };
        sink.push_bytes(&placeholder[..n]);
        return;
    }
    let mut len = 0usize;
    let mut q = s as *const u8;
    let max = if spec.has_precision { spec.precision } else { 4096 };
    while len < max && unsafe { *q } != 0 {
        len += 1;
        q = unsafe { q.add(1) };
    }
    let bytes = unsafe { core::slice::from_raw_parts(s as *const u8, len) };
    let pad = spec.width.saturating_sub(len);
    for _ in 0..pad {
        sink.push(b' ');
    }
    sink.push_bytes(bytes);
}

fn emit_signed(sink: &mut Sink<'_>, v: i64, spec: &Spec) {
    let mut buf = [0u8; MAX_NUMBUF];
    let (neg, mag) = if v < 0 { (true, (v as i128).unsigned_abs() as u64) } else { (false, v as u64) };
    let mut n = format_uint(&mut buf, mag, 10, false);
    if neg {
        n += 1;
        buf[MAX_NUMBUF - n] = b'-';
    }
    apply_pad_and_emit(sink, &buf[MAX_NUMBUF - n..], spec, neg);
}

fn emit_unsigned_dec(sink: &mut Sink<'_>, v: u64, spec: &Spec) {
    let mut buf = [0u8; MAX_NUMBUF];
    let n = format_uint(&mut buf, v, 10, false);
    apply_pad_and_emit(sink, &buf[MAX_NUMBUF - n..], spec, false);
}

fn emit_hex(sink: &mut Sink<'_>, v: u64, spec: &Spec, upper: bool, prefix_0x: bool) {
    let mut buf = [0u8; MAX_NUMBUF];
    let mut n = format_uint(&mut buf, v, 16, upper);
    if prefix_0x {
        n += 2;
        buf[MAX_NUMBUF - n] = b'0';
        buf[MAX_NUMBUF - n + 1] = b'x';
    }
    apply_pad_and_emit(sink, &buf[MAX_NUMBUF - n..], spec, prefix_0x);
}

fn apply_pad_and_emit(sink: &mut Sink<'_>, payload: &[u8], spec: &Spec, has_prefix: bool) {
    let pad = spec.width.saturating_sub(payload.len());
    if spec.zero_pad && !spec.has_precision {
        // For numeric '0' padding, the prefix (- or 0x) goes
        // before the zeros.
        if has_prefix && payload.len() >= 2 && (payload[0] == b'-' || payload[0] == b'0') {
            // emit prefix first
            let prefix_len = if payload[0] == b'-' { 1 } else { 2 };
            sink.push_bytes(&payload[..prefix_len]);
            for _ in 0..pad {
                sink.push(b'0');
            }
            sink.push_bytes(&payload[prefix_len..]);
        } else {
            for _ in 0..pad {
                sink.push(b'0');
            }
            sink.push_bytes(payload);
        }
    } else {
        for _ in 0..pad {
            sink.push(b' ');
        }
        sink.push_bytes(payload);
    }
}

/// Format an unsigned integer into the *tail* of `buf` (no leading
/// zeros; lowest digit at the end).  Returns the digit count.
fn format_uint(buf: &mut [u8; MAX_NUMBUF], mut v: u64, base: u64, upper: bool) -> usize {
    if v == 0 {
        buf[MAX_NUMBUF - 1] = b'0';
        return 1;
    }
    let alphabet: &[u8] = if upper {
        b"0123456789ABCDEF"
    } else {
        b"0123456789abcdef"
    };
    let mut n = 0;
    while v != 0 {
        n += 1;
        let d = (v % base) as usize;
        buf[MAX_NUMBUF - n] = alphabet[d];
        v /= base;
    }
    n
}
