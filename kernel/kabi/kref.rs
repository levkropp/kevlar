// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux `struct kref` shim — atomic reference count handle.
//!
//! `kref_put` is the only operation that takes a release callback;
//! when the count drops to 0, the callback fires and the refcount
//! is conceptually dead.  Callers transmute the kref back to its
//! containing struct via `container_of`.

use core::sync::atomic::{AtomicI32, Ordering};

use crate::ksym;

#[repr(C)]
pub struct KrefShim {
    pub refcount: AtomicI32,
}

#[unsafe(no_mangle)]
pub extern "C" fn kref_init(k: *mut KrefShim) {
    if k.is_null() {
        return;
    }
    unsafe {
        (*k).refcount.store(1, Ordering::Release);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kref_get(k: *mut KrefShim) {
    if k.is_null() {
        return;
    }
    unsafe {
        (*k).refcount.fetch_add(1, Ordering::Relaxed);
    }
}

/// Decrement; if it hits 0, invoke `release(k)` and return 1.
#[unsafe(no_mangle)]
pub extern "C" fn kref_put(
    k: *mut KrefShim,
    release: Option<extern "C" fn(*mut KrefShim)>,
) -> i32 {
    if k.is_null() {
        return 0;
    }
    let prev = unsafe { (*k).refcount.fetch_sub(1, Ordering::AcqRel) };
    if prev == 1 {
        if let Some(r) = release {
            r(k);
        }
        return 1;
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn kref_read(k: *const KrefShim) -> u32 {
    if k.is_null() {
        return 0;
    }
    unsafe { (*k).refcount.load(Ordering::Acquire) as u32 }
}

ksym!(kref_init);
ksym!(kref_get);
ksym!(kref_put);
ksym!(kref_read);
