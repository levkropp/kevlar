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
.global usercopy_memcpy
usercopy_memcpy:
    cbz     x2, 2f
1:
usercopy3:
    ldrb    w3, [x1], #1
    strb    w3, [x0], #1
    subs    x2, x2, #1
    b.ne    1b
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
