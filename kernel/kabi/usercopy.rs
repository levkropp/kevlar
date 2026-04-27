// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! User-pointer access shims (K4 simplification).
//!
//! Linux's `copy_to_user` / `copy_from_user` translate a userspace
//! pointer into a kernel-safe access (page-table walk, fault
//! handling).  K4 stages reads/writes through kernel staging
//! buffers, so the "user pointer" the fops callback sees is
//! actually a kernel pointer — these become memcpy.
//!
//! K5 adds the real UserVAddr-aware path when the first
//! userspace-driven I/O hits a fop.  Both versions return
//! "bytes NOT copied" (Linux convention: 0 = full success).

use core::ffi::c_void;

use crate::ksym_named;

// Internal Rust names use the `kabi_` prefix to avoid clashing with
// the platform's already-existing `copy_to_user` / `copy_from_user`
// asm symbols.  Modules see the canonical Linux names through the
// ksym_named exports below.

pub extern "C" fn kabi_copy_to_user(
    to: *mut c_void,
    from: *const c_void,
    n: usize,
) -> usize {
    if to.is_null() || from.is_null() || n == 0 {
        return 0;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(from as *const u8, to as *mut u8, n);
    }
    0
}

pub extern "C" fn kabi_copy_from_user(
    to: *mut c_void,
    from: *const c_void,
    n: usize,
) -> usize {
    if to.is_null() || from.is_null() || n == 0 {
        return 0;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(from as *const u8, to as *mut u8, n);
    }
    0
}

pub extern "C" fn kabi_clear_user(to: *mut c_void, n: usize) -> usize {
    if to.is_null() || n == 0 {
        return 0;
    }
    unsafe {
        core::ptr::write_bytes(to as *mut u8, 0, n);
    }
    0
}

pub extern "C" fn kabi_strnlen_user(s: *const u8, n: usize) -> usize {
    if s.is_null() {
        return 0;
    }
    let mut i = 0usize;
    while i < n {
        if unsafe { *s.add(i) } == 0 {
            return i + 1;
        }
        i += 1;
    }
    n + 1
}

ksym_named!("copy_to_user", kabi_copy_to_user);
ksym_named!("copy_from_user", kabi_copy_from_user);
ksym_named!("clear_user", kabi_clear_user);
ksym_named!("strnlen_user", kabi_strnlen_user);
