// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Pod (Plain Old Data) trait for safe type punning across the user-kernel boundary.

use core::mem::size_of;

/// A type where any bit pattern is a valid value.
///
/// # Safety
/// Only implement for types that are `Copy`, `repr(C)` (or primitive),
/// and have no padding bytes that could leak information.
pub unsafe trait Pod: Copy + 'static {}

// Implement for primitive types.
unsafe impl Pod for u8 {}
unsafe impl Pod for u16 {}
unsafe impl Pod for u32 {}
unsafe impl Pod for u64 {}
unsafe impl Pod for usize {}
unsafe impl Pod for i8 {}
unsafe impl Pod for i16 {}
unsafe impl Pod for i32 {}
unsafe impl Pod for i64 {}
unsafe impl Pod for isize {}

/// Interpret a Pod value as a byte slice.
pub fn as_bytes<T: Pod>(value: &T) -> &[u8] {
    unsafe { core::slice::from_raw_parts(value as *const T as *const u8, size_of::<T>()) }
}

/// Interpret a byte buffer as a reference to a Pod type.
///
/// Returns `None` if the buffer is too small.
pub fn from_bytes<T: Pod>(bytes: &[u8]) -> Option<&T> {
    if bytes.len() < size_of::<T>() {
        return None;
    }
    Some(unsafe { &*(bytes.as_ptr() as *const T) })
}

/// Read a `Copy` value from a byte slice at the given offset.
///
/// This encapsulates the unsafe pointer-cast pattern for reading structured
/// data from raw byte buffers (e.g., user-provided buffers, file contents).
///
/// # Panics
/// Panics if `offset + size_of::<T>() > slice.len()`.
pub fn read_copy_from_slice<T: Copy>(slice: &[u8], offset: usize) -> T {
    assert!(offset + size_of::<T>() <= slice.len());
    unsafe {
        let mut val = core::mem::MaybeUninit::<T>::uninit();
        core::ptr::copy_nonoverlapping(
            slice.as_ptr().add(offset),
            val.as_mut_ptr() as *mut u8,
            size_of::<T>(),
        );
        val.assume_init()
    }
}

/// Interpret a `Copy` value as a byte slice.
///
/// Like `as_bytes` but works with any `Copy` type, not just `Pod`.
pub fn copy_as_bytes<T: Copy>(value: &T) -> &[u8] {
    unsafe { core::slice::from_raw_parts(value as *const T as *const u8, size_of::<T>()) }
}

/// Interpret the start of a byte buffer as a reference to a `Copy` type.
///
/// Returns `None` if the buffer is too small.
pub fn ref_from_prefix<T: Copy>(bytes: &[u8]) -> Option<&T> {
    if bytes.len() < size_of::<T>() {
        return None;
    }
    Some(unsafe { &*(bytes.as_ptr() as *const T) })
}

/// Interpret the start of a byte buffer as a slice of `Copy` values.
///
/// Returns `None` if the buffer is too small for `count` elements.
pub fn slice_from_prefix<T: Copy>(bytes: &[u8], count: usize) -> Option<&[T]> {
    let needed = size_of::<T>().checked_mul(count)?;
    if bytes.len() < needed {
        return None;
    }
    Some(unsafe { core::slice::from_raw_parts(bytes.as_ptr() as *const T, count) })
}

/// Cast a `&str` to a `&T` where `T` is a `#[repr(transparent)]` wrapper around `str`.
///
/// This is safe because `repr(transparent)` guarantees identical layout
/// and both are fat pointers with identical metadata.
///
/// # Safety (internal)
/// The caller must ensure T is `#[repr(transparent)]` over `str`.
pub fn str_newtype_ref<T: ?Sized>(s: &str) -> &T {
    // Both &str and &T are fat pointers (ptr + len). repr(transparent) guarantees
    // the same layout, so transmuting the fat pointer is valid.
    unsafe { core::mem::transmute_copy::<&str, &T>(&s) }
}

/// Interpret a byte buffer as a slice of Pod values.
///
/// Returns `None` if the buffer is too small for at least one element.
pub fn slice_from_bytes<T: Pod>(bytes: &[u8], count: usize) -> Option<&[T]> {
    let needed = size_of::<T>().checked_mul(count)?;
    if bytes.len() < needed {
        return None;
    }
    Some(unsafe { core::slice::from_raw_parts(bytes.as_ptr() as *const T, count) })
}
