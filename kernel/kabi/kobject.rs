// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux `struct kobject` shim — minimal K3 surface, no sysfs.
//!
//! Modules see kobjects as opaque pointers.  Internally we hang an
//! atomic refcount + an optional name on each.  K3 is only deep
//! enough to satisfy `device_initialize`; sysfs / kobject_uevent /
//! ksets defer to a future milestone when something actually needs
//! them.

use alloc::boxed::Box;
use alloc::string::String;
use core::ffi::c_char;
use core::sync::atomic::{AtomicI32, Ordering};

use kevlar_platform::spinlock::SpinLock;

use crate::ksym;

/// Heap-allocated state behind a `struct kobject`.  Reachable
/// through the `_kevlar_inner` slot of the C struct.
pub struct KobjectInner {
    pub refcount: AtomicI32,
    pub name: SpinLock<Option<String>>,
}

impl KobjectInner {
    pub fn new() -> Self {
        Self {
            refcount: AtomicI32::new(1),
            name: SpinLock::new(None),
        }
    }
}

#[repr(C)]
pub struct KobjectShim {
    pub inner: *mut KobjectInner,
}

/// Allocate (if needed) the per-kobject state.  Idempotent — safe
/// to call multiple times.
pub fn ensure_inner(k: *mut KobjectShim) -> *mut KobjectInner {
    if k.is_null() {
        return core::ptr::null_mut();
    }
    let existing = unsafe { (*k).inner };
    if !existing.is_null() {
        return existing;
    }
    let boxed = Box::new(KobjectInner::new());
    let raw = Box::into_raw(boxed);
    unsafe {
        (*k).inner = raw;
    }
    raw
}

#[unsafe(no_mangle)]
pub extern "C" fn kobject_init(k: *mut KobjectShim, _ktype: *const ()) {
    let _ = ensure_inner(k);
}

#[unsafe(no_mangle)]
pub extern "C" fn kobject_get(k: *mut KobjectShim) -> *mut KobjectShim {
    if k.is_null() {
        return k;
    }
    let inner = ensure_inner(k);
    unsafe {
        (*inner).refcount.fetch_add(1, Ordering::Relaxed);
    }
    k
}

#[unsafe(no_mangle)]
pub extern "C" fn kobject_put(k: *mut KobjectShim) {
    if k.is_null() {
        return;
    }
    let inner = unsafe { (*k).inner };
    if inner.is_null() {
        return;
    }
    let prev = unsafe { (*inner).refcount.fetch_sub(1, Ordering::AcqRel) };
    if prev == 1 {
        // K3: leak the inner.  Real Linux frees the embedded
        // kobject's containing struct via release().
        // Without sysfs / ktype callbacks we have nothing useful
        // to do here yet.
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kobject_set_name(
    k: *mut KobjectShim,
    name: *const c_char,
) -> i32 {
    if k.is_null() || name.is_null() {
        return -22; // -EINVAL
    }
    let inner = ensure_inner(k);
    let name_str = unsafe { c_str_to_string(name) };
    unsafe {
        *(*inner).name.lock() = Some(name_str);
    }
    0
}

/// Read a NUL-terminated C string into an owned String.  Capped at
/// 256 bytes to bound a malformed module's mistake.
#[allow(dead_code)]
unsafe fn c_str_to_string(p: *const c_char) -> String {
    let mut len = 0usize;
    while len < 256 && unsafe { *p.add(len) } != 0 {
        len += 1;
    }
    let bytes = unsafe { core::slice::from_raw_parts(p as *const u8, len) };
    String::from_utf8_lossy(bytes).into_owned()
}

#[unsafe(no_mangle)]
pub extern "C" fn kobject_add(
    _k: *mut KobjectShim,
    _parent: *mut KobjectShim,
    _name: *const c_char,
) -> i32 {
    // K3: no sysfs.  Always succeed.
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn kobject_del(_k: *mut KobjectShim) {
    // K3: no sysfs.
}

/// Allocate + return a `struct kobject *` named `name`, parented at
/// `parent`.  Phase 10 ext4-arc: ext4_init_sysfs calls this to create
/// `/sys/fs/ext4`; if NULL it bails with -ENOMEM.  We return a small
/// heap-allocated `KobjectShim` that satisfies the non-null check
/// without wiring into a real sysfs tree.
#[unsafe(no_mangle)]
pub extern "C" fn kobject_create_and_add(
    name: *const c_char, _parent: *mut KobjectShim,
) -> *mut KobjectShim {
    let boxed = Box::new(KobjectShim { inner: core::ptr::null_mut() });
    let raw = Box::into_raw(boxed);
    let _ = ensure_inner(raw);
    if !name.is_null() {
        let len = unsafe {
            let mut n = 0usize;
            while *name.add(n) != 0 && n < 64 { n += 1; }
            n
        };
        let bytes = unsafe { core::slice::from_raw_parts(name as *const u8, len) };
        if let Ok(s) = core::str::from_utf8(bytes) {
            let inner = unsafe { (*raw).inner };
            if !inner.is_null() {
                let mut name_lock = unsafe { (*inner).name.lock() };
                *name_lock = Some(String::from(s));
            }
        }
    }
    raw
}

ksym!(kobject_init);
ksym!(kobject_get);
ksym!(kobject_put);
ksym!(kobject_set_name);
ksym!(kobject_add);
ksym!(kobject_del);
ksym!(kobject_create_and_add);
