// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! `memcpy` / `memcpy_toio` kABI exports.
//!
//! `memcpy` is implemented in `platform/mem.rs` as a `no_mangle`
//! extern "C" function — the symbol exists in our linked binary.
//! We just need a Rust-visible reference so `ksym!` can take its
//! address.  The `extern "C"` block reaches it without depending
//! on the platform module's Rust-side visibility.
//!
//! `memcpy_toio` handles the "copy to memory-mapped IO" case — on
//! aarch64 it's the same as memcpy at our level (IO accesses go
//! through plain ldr/str; no special barrier required for the
//! K15 stub corpus).

use core::ffi::c_int;

unsafe extern "C" {
    fn memcpy(dest: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dest: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dest: *mut u8, c: c_int, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> c_int;
    fn strlen(s: *const u8) -> usize;
}

crate::ksym!(memcpy);
crate::ksym_named!("memcpy_toio", memcpy);
crate::ksym!(memmove);
crate::ksym!(memset);
crate::ksym!(memcmp);
crate::ksym!(strlen);

// ── Linux-named string helpers — implementations live in this
// file because platform/mem.rs only ships the four core C-runtime
// primitives.  Linux fs code calls strncmp/strcmp/strnlen/strcspn
// + a handful of "kernel-flavored" string helpers (strscpy,
// skip_spaces, memchr_inv, kstrdup-style allocators) that we
// implement here in safe-Rust.

/// strncmp — compare up to `n` bytes.  Stops at first NUL or
/// difference.  Returns 0 on equal, negative if a<b, positive if
/// a>b (matching libc convention via i8 sign extension).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn strncmp(a: *const u8, b: *const u8, n: usize) -> c_int {
    if a.is_null() || b.is_null() {
        return 0;
    }
    for i in 0..n {
        let ca = unsafe { *a.add(i) };
        let cb = unsafe { *b.add(i) };
        if ca != cb {
            return (ca as c_int) - (cb as c_int);
        }
        if ca == 0 {
            return 0;
        }
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn strcmp(a: *const u8, b: *const u8) -> c_int {
    if a.is_null() || b.is_null() {
        return 0;
    }
    let mut i = 0usize;
    loop {
        let ca = unsafe { *a.add(i) };
        let cb = unsafe { *b.add(i) };
        if ca != cb {
            return (ca as c_int) - (cb as c_int);
        }
        if ca == 0 {
            return 0;
        }
        i += 1;
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn strnlen(s: *const u8, n: usize) -> usize {
    if s.is_null() {
        return 0;
    }
    for i in 0..n {
        if unsafe { *s.add(i) } == 0 {
            return i;
        }
    }
    n
}

/// strcspn — return length of the initial segment of `s` not
/// containing any byte from `reject`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn strcspn(s: *const u8, reject: *const u8) -> usize {
    if s.is_null() || reject.is_null() {
        return 0;
    }
    let rlen = unsafe { strlen(reject) };
    let mut i = 0usize;
    loop {
        let c = unsafe { *s.add(i) };
        if c == 0 {
            return i;
        }
        for j in 0..rlen {
            if c == unsafe { *reject.add(j) } {
                return i;
            }
        }
        i += 1;
    }
}

/// memchr_inv — find first byte that is NOT equal to `c`.
/// Used by ext4 / others to validate "is this region all-zero?".
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memchr_inv(s: *const u8, c: c_int,
                                    n: usize) -> *const u8 {
    if s.is_null() {
        return core::ptr::null();
    }
    let target = c as u8;
    for i in 0..n {
        if unsafe { *s.add(i) } != target {
            return unsafe { s.add(i) };
        }
    }
    core::ptr::null()
}

/// skip_spaces — skip ASCII whitespace at the front of a string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skip_spaces(s: *const u8) -> *const u8 {
    if s.is_null() {
        return s;
    }
    let mut p = s;
    loop {
        let c = unsafe { *p };
        if c == 0 {
            return p;
        }
        if !matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c) {
            return p;
        }
        p = unsafe { p.add(1) };
    }
}

crate::ksym!(strncmp);
crate::ksym!(strcmp);
crate::ksym!(strnlen);
crate::ksym!(strcspn);
crate::ksym!(memchr_inv);
crate::ksym!(skip_spaces);

// `sized_strscpy` is Linux's bounded string-copy with NUL
// termination guarantee.  Returns the number of bytes copied
// (excluding NUL), or -E2BIG if truncated.  We implement it on
// top of strnlen + memcpy.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sized_strscpy(dest: *mut u8, src: *const u8,
                                       count: usize) -> isize {
    if dest.is_null() || src.is_null() || count == 0 {
        return -22; // -EINVAL
    }
    let src_len = unsafe { strnlen(src, count) };
    let copy_len = if src_len < count { src_len } else { count - 1 };
    unsafe {
        core::ptr::copy_nonoverlapping(src, dest, copy_len);
        *dest.add(copy_len) = 0;
    }
    if src_len >= count {
        -7 // -E2BIG
    } else {
        copy_len as isize
    }
}

crate::ksym!(sized_strscpy);

// `kstrtoull` parses an unsigned long long from a NUL-terminated
// C string with optional 0x prefix (when base==0 or base==16).
// On success writes to *res and returns 0; on parse error
// returns -EINVAL.  v1 implementation handles base 0/10/16 only —
// covers everything erofs / fs-options care about.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kstrtoull(s: *const u8, base: u32,
                                   res: *mut u64) -> c_int {
    if s.is_null() || res.is_null() {
        return -22;
    }
    let mut p = s;
    let mut b = base;
    if b == 0 {
        // Auto-detect: 0x → 16, leading 0 → 8, else 10.
        if unsafe { *p } == b'0' && (unsafe { *p.add(1) } == b'x' ||
                                     unsafe { *p.add(1) } == b'X') {
            p = unsafe { p.add(2) };
            b = 16;
        } else if unsafe { *p } == b'0' {
            b = 8;
        } else {
            b = 10;
        }
    } else if b == 16
        && unsafe { *p } == b'0'
        && (unsafe { *p.add(1) } == b'x' || unsafe { *p.add(1) } == b'X')
    {
        p = unsafe { p.add(2) };
    }
    let mut acc: u64 = 0;
    let mut digits = 0usize;
    loop {
        let c = unsafe { *p };
        let d = match c {
            b'0'..=b'9' => (c - b'0') as u32,
            b'a'..=b'f' => 10 + (c - b'a') as u32,
            b'A'..=b'F' => 10 + (c - b'A') as u32,
            _ => break,
        };
        if d >= b {
            break;
        }
        let next = acc.checked_mul(b as u64).and_then(|v| v.checked_add(d as u64));
        match next {
            Some(v) => acc = v,
            None => return -34, // -ERANGE
        }
        p = unsafe { p.add(1) };
        digits += 1;
    }
    if digits == 0 {
        return -22;
    }
    // Allow trailing whitespace / NUL.
    while !matches!(unsafe { *p }, 0 | b'\n' | b' ' | b'\t' | b'\r') {
        return -22;
    }
    unsafe { *res = acc };
    0
}

crate::ksym!(kstrtoull);

// `__fortify_panic` is the bail-out that GCC's _FORTIFY_SOURCE=2
// inserts when a compile-time-known-too-small buffer is detected.
// We make it a panic; modules shouldn't reach it under normal
// operation.
#[unsafe(no_mangle)]
pub extern "C" fn __fortify_panic(_func: *const u8) -> ! {
    panic!("kabi: __fortify_panic from a loaded module");
}

crate::ksym!(__fortify_panic);

// `__check_object_size` is Linux's CONFIG_HARDENED_USERCOPY hook.
// Validates that copy_to/from_user pointers are within the
// expected slab/stack region.  We don't implement that hardening
// yet — the existing kabi_copy_to_user / kabi_copy_from_user
// already do bounds-checking.  No-op stub.
#[unsafe(no_mangle)]
pub extern "C" fn __check_object_size(_ptr: *const core::ffi::c_void,
                                      _n: usize, _to_user: bool) {}

crate::ksym!(__check_object_size);

// `validate_usercopy_range` — newer Linux helper added for
// SLUB_DEBUG.  No-op.
#[unsafe(no_mangle)]
pub extern "C" fn validate_usercopy_range(_ptr: *const core::ffi::c_void,
                                          _n: usize) -> bool {
    true
}

crate::ksym!(validate_usercopy_range);
