// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux raw spinlock primitives.
//!
//! K12: no-op (single-threaded module-init context; no
//! contention to actually lock against).  When K13+ surfaces a
//! driver whose probe runs concurrently with anything, we'll
//! revisit with real arch-spinlock primitives.

use core::ffi::c_void;

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn _raw_spin_lock_irqsave(_lock: *mut c_void) -> u64 {
    // Linux returns the saved IRQ flags through the return value
    // (the macro `spin_lock_irqsave(lock, flags)` writes the
    // result into `flags`).  K12: 0.
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn _raw_spin_unlock_irqrestore(_lock: *mut c_void, _flags: u64) {}

#[unsafe(no_mangle)]
pub extern "C" fn _raw_spin_lock(_lock: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn _raw_spin_unlock(_lock: *mut c_void) {}

ksym!(_raw_spin_lock_irqsave);
ksym!(_raw_spin_unlock_irqrestore);
ksym!(_raw_spin_lock);
ksym!(_raw_spin_unlock);
