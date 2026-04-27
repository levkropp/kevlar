// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux `wait_queue_head` shims for K2 modules.
//!
//! Modules see this as an opaque struct holding one pointer:
//!
//! ```c
//! struct wait_queue_head { void *_kevlar_inner; };
//! ```
//!
//! `init_waitqueue_head` heap-allocates a Kevlar `WaitQueue` and
//! stores the pointer in the shim.  `wake_up*` and the
//! `kabi_wait_event` helper walk through that pointer to the real
//! Kevlar primitive.

use alloc::boxed::Box;
use core::ffi::c_void;

use crate::ksym;
use crate::process::wait_queue::WaitQueue;

#[repr(C)]
pub struct WaitQueueHeadShim {
    pub inner: *mut WaitQueue,
}

#[allow(unsafe_code)]
fn deref_shim<'a>(wq: *mut WaitQueueHeadShim) -> Option<&'a WaitQueue> {
    if wq.is_null() {
        return None;
    }
    let inner = unsafe { (*wq).inner };
    if inner.is_null() {
        return None;
    }
    Some(unsafe { &*inner })
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn init_waitqueue_head(wq: *mut WaitQueueHeadShim) {
    if wq.is_null() {
        return;
    }
    let boxed = Box::new(WaitQueue::new());
    unsafe {
        (*wq).inner = Box::into_raw(boxed);
    }
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn destroy_waitqueue_head(wq: *mut WaitQueueHeadShim) {
    if wq.is_null() {
        return;
    }
    let inner = unsafe { (*wq).inner };
    if !inner.is_null() {
        unsafe {
            drop(Box::from_raw(inner));
            (*wq).inner = core::ptr::null_mut();
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn wake_up(wq: *mut WaitQueueHeadShim) {
    if let Some(inner) = deref_shim(wq) {
        inner.wake_one();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn wake_up_all(wq: *mut WaitQueueHeadShim) {
    if let Some(inner) = deref_shim(wq) {
        inner.wake_all();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn wake_up_interruptible(wq: *mut WaitQueueHeadShim) {
    if let Some(inner) = deref_shim(wq) {
        inner.wake_one();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn wake_up_interruptible_all(wq: *mut WaitQueueHeadShim) {
    if let Some(inner) = deref_shim(wq) {
        inner.wake_all();
    }
}

/// Sleep on `wq` until `cond(arg)` returns non-zero.  Returns 0 on
/// success, -EINTR on signal interruption.
///
/// Linux's `wait_event_interruptible(wq, cond)` macro expands to a
/// loop around the same idea.  K2 modules call this shim directly.
#[unsafe(no_mangle)]
pub extern "C" fn kabi_wait_event(
    wq: *mut WaitQueueHeadShim,
    cond: extern "C" fn(*mut c_void) -> i32,
    arg: *mut c_void,
) -> i32 {
    let inner = match deref_shim(wq) {
        Some(i) => i,
        None => return -22, // -EINVAL
    };
    let result = inner.sleep_signalable_until(|| {
        let r = cond(arg);
        if r != 0 {
            Ok(Some(()))
        } else {
            Ok(None)
        }
    });
    match result {
        Ok(()) => 0,
        Err(_) => -4, // -EINTR
    }
}

ksym!(init_waitqueue_head);
ksym!(destroy_waitqueue_head);
ksym!(wake_up);
ksym!(wake_up_all);
ksym!(wake_up_interruptible);
ksym!(wake_up_interruptible_all);
ksym!(kabi_wait_event);
