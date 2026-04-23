// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// ARM64 user-space memory access helpers.
// These are called from Rust and may fault on bad user addresses.
// The page fault handler checks if the faulting PC matches one of these
// labels and, if so, returns an error instead of panicking.

.global usercopy1
.global usercopy2
.global usercopy3

// Usercopy region start — any fault in [usercopy_start, usercopy_end)
// is a user-space page fault, not a kernel bug.
.global usercopy_start
usercopy_start:

// Copy one byte from user space: usercopy_load_u8(src: *const u8) -> u8
.global usercopy_load_u8
usercopy_load_u8:
usercopy1:
    ldrb    w0, [x0]
    ret

// Store one byte to user space: usercopy_store_u8(dst: *mut u8, val: u8)
.global usercopy_store_u8
usercopy_store_u8:
usercopy2:
    strb    w1, [x0]
    ret

// Copy bytes from user to kernel or kernel to user.
// usercopy_memcpy(dst: *mut u8, src: *const u8, len: usize)
//
// Bulk-copy 32 bytes per iteration via ldp/stp (pair of 128-bit registers
// would require FP; use 64-bit pair for no-neon kernels — two ldp/stp of
// 64-bit pairs per iteration = 32 bytes with a 4-wide pipeline).  Tail is
// byte-by-byte.  Any fault inside [usercopy_start, usercopy_end) is
// caught by the page-fault handler and mapped to an AccessError; no
// specific label needed beyond the range check.
.global usercopy_memcpy
usercopy_memcpy:
    cbz     x2, 2f
    // Fast path: ≥32 bytes remaining → bulk 32-byte copies.
    cmp     x2, #32
    b.lo    3f
usercopy3:
1:  ldp     x3, x4, [x1], #16
    ldp     x5, x6, [x1], #16
    stp     x3, x4, [x0], #16
    stp     x5, x6, [x0], #16
    sub     x2, x2, #32
    cmp     x2, #32
    b.hs    1b
3:  // Tail: < 32 bytes.  Try 8-byte chunks first, then byte-by-byte.
    cmp     x2, #8
    b.lo    5f
4:  ldr     x3, [x1], #8
    str     x3, [x0], #8
    sub     x2, x2, #8
    cmp     x2, #8
    b.hs    4b
5:  cbz     x2, 2f
6:  ldrb    w3, [x1], #1
    strb    w3, [x0], #1
    subs    x2, x2, #1
    b.ne    6b
2:
    ret

// void copy_from_user(void *dst, const void *src, size_t len)
// void copy_to_user(void *dst, const void *src, size_t len)
// These are aliases for usercopy_memcpy so faults are caught by usercopy3.
.global copy_from_user
.global copy_to_user
.set copy_from_user, usercopy_memcpy
.set copy_to_user, usercopy_memcpy

// size_t strncpy_from_user(void *dst, const void *src, size_t max_len)
.global strncpy_from_user
.global usercopy4
strncpy_from_user:
    mov     x3, x2          // save original max_len
    cbz     x2, 6f
usercopy4:
5:  ldrb    w4, [x1], #1
    cbz     w4, 6f
    strb    w4, [x0], #1
    subs    x2, x2, #1
    b.ne    5b
6:  sub     x0, x3, x2      // return copied count
    ret

// void memset_user(void *dst, uint8_t value, size_t len)
.global memset_user
.global usercopy5
memset_user:
    cbz     x2, 8f
usercopy5:
7:  strb    w1, [x0], #1
    subs    x2, x2, #1
    b.ne    7b
8:  ret

.global usercopy_end
usercopy_end:
