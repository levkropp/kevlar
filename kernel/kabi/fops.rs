// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux `struct file_operations` + `struct file` + `struct inode`
//! shims for K4.
//!
//! Modules see these as #[repr(C)] structs at fixed offsets.
//! Drivers commonly read/write `file->private_data` (per-open
//! state slot) and `file->f_pos` (current offset) — those live
//! at known offsets matching the C header.

use core::ffi::c_void;

#[repr(C)]
pub struct FileShim {
    pub _kevlar_inner: *mut c_void,
    pub private_data: *mut c_void,
    pub f_pos: i64,
    pub f_flags: u32,
    pub _pad: u32,
}

#[repr(C)]
pub struct InodeShim {
    pub _kevlar_inner: *mut c_void,
    pub i_rdev: u32,
    pub _pad: u32,
    pub i_size: i64,
}

/// Function-pointer table that the module fills in and we walk
/// when dispatching from Kevlar's `FileLike` trait.  Slots not
/// supported by K4 (mmap, poll) are still here at the right
/// offsets so K5+ can wire them in without breaking layouts.
#[repr(C)]
pub struct FileOperationsShim {
    pub owner: *const c_void,
    pub llseek: Option<extern "C" fn(*mut FileShim, i64, i32) -> i64>,
    pub read: Option<
        extern "C" fn(*mut FileShim, *mut u8, usize, *mut i64) -> isize,
    >,
    pub write: Option<
        extern "C" fn(*mut FileShim, *const u8, usize, *mut i64) -> isize,
    >,
    pub unlocked_ioctl: Option<
        extern "C" fn(*mut FileShim, u32, usize) -> isize,
    >,
    pub poll: Option<extern "C" fn(*mut FileShim, *const c_void) -> u32>,
    pub mmap: Option<extern "C" fn(*mut FileShim, *const c_void) -> i32>,
    pub open: Option<
        extern "C" fn(*mut InodeShim, *mut FileShim) -> i32,
    >,
    pub release: Option<
        extern "C" fn(*mut InodeShim, *mut FileShim) -> i32,
    >,
}
