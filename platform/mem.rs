// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Custom mem* functions for the kernel.
//!
//! We provide our own implementations instead of using compiler-builtins-mem,
//! because the compiler-builtins implementations use u128/SSE operations
//! which are not available in our no-SSE kernel target.

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcpy(dest: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    let mut i = 0;
    while i + 8 <= n {
        unsafe {
            (dest.add(i) as *mut u64).write_unaligned((src.add(i) as *const u64).read_unaligned());
        }
        i += 8;
    }
    while i < n {
        unsafe {
            *dest.add(i) = *src.add(i);
        }
        i += 1;
    }
    dest
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memmove(dest: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    if (dest as usize) <= (src as usize) {
        unsafe { memcpy(dest, src, n) }
    } else {
        let mut i = n;
        while i > 0 {
            i -= 1;
            unsafe {
                *dest.add(i) = *src.add(i);
            }
        }
        dest
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memset(dest: *mut u8, c: i32, n: usize) -> *mut u8 {
    let byte = c as u8;
    let word = (byte as u64)
        | (byte as u64) << 8
        | (byte as u64) << 16
        | (byte as u64) << 24
        | (byte as u64) << 32
        | (byte as u64) << 40
        | (byte as u64) << 48
        | (byte as u64) << 56;
    let mut i = 0;
    while i + 8 <= n {
        unsafe {
            (dest.add(i) as *mut u64).write_unaligned(word);
        }
        i += 8;
    }
    while i < n {
        unsafe {
            *dest.add(i) = byte;
        }
        i += 1;
    }
    dest
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    let mut i = 0;
    while i < n {
        let av = unsafe { *a.add(i) };
        let bv = unsafe { *b.add(i) };
        if av != bv {
            return (av as i32) - (bv as i32);
        }
        i += 1;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn bcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    unsafe { memcmp(a, b, n) }
}
