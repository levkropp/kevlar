// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux kernel `mutex` shim.
//!
//! The real mutex is a sleepable lock with priority inheritance and
//! adaptive spinning.  K15 stubs to no-op: every K1-K14 module-load
//! path runs in single-threaded init context, so contention is
//! impossible and ordering doesn't matter.
//!
//! K20+ when probe paths run on real threads, we route to either
//! a `SpinLock` (cheap path) or a real sleepable mutex.

use core::ffi::c_void;

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn mutex_lock(_lock: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn mutex_unlock(_lock: *mut c_void) {}

ksym!(mutex_lock);
ksym!(mutex_unlock);
