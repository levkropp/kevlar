// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Wait-wound mutex (`ww_mutex`) shim.
//!
//! Linux's wait-wound mutex is a deadlock-avoiding nestable lock used
//! by DRM (and BTRFS) for transactional resource acquisition.  K14
//! stubs all operations to no-op / success: the DRM-helper modules
//! that reference these symbols don't fire any locking at load time
//! (no probe runs), and Kevlar's init context is single-threaded so
//! contention is impossible.
//!
//! The `reservation_ww_class` static is the global ww_class instance
//! for `dma_resv`.  We expose a 64-byte zero buffer; the real Linux
//! struct is ~88 bytes with internal mutex tracking.  Callers don't
//! dereference fields at K14, so the layout doesn't yet matter.

use core::ffi::c_void;

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn ww_mutex_lock(
    _lock: *mut c_void,
    _ctx: *mut c_void,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn ww_mutex_lock_interruptible(
    _lock: *mut c_void,
    _ctx: *mut c_void,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn ww_mutex_unlock(_lock: *mut c_void) {}

ksym!(ww_mutex_lock);
ksym!(ww_mutex_lock_interruptible);
ksym!(ww_mutex_unlock);

#[unsafe(no_mangle)]
pub static reservation_ww_class: [u8; 64] = [0; 64];
crate::ksym_static!(reservation_ww_class);
