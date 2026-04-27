// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux `completion` shims for K2 modules.
//!
//! Modules see this as an opaque struct holding one pointer:
//!
//! ```c
//! struct completion { void *_kevlar_inner; };
//! ```
//!
//! Backed by a single-flag wait_queue.  `complete()` sets the flag
//! and wakes one sleeper; `complete_all()` sets and wakes everyone.

use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::ksym;
use crate::process::wait_queue::WaitQueue;

pub struct CompletionInner {
    flag: AtomicBool,
    wq: WaitQueue,
}

impl CompletionInner {
    pub fn new() -> Self {
        Self {
            flag: AtomicBool::new(false),
            wq: WaitQueue::new(),
        }
    }
}

#[repr(C)]
pub struct CompletionShim {
    pub inner: *mut CompletionInner,
}

#[allow(unsafe_code)]
fn deref_shim<'a>(c: *mut CompletionShim) -> Option<&'a CompletionInner> {
    if c.is_null() {
        return None;
    }
    let inner = unsafe { (*c).inner };
    if inner.is_null() {
        return None;
    }
    Some(unsafe { &*inner })
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn init_completion(c: *mut CompletionShim) {
    if c.is_null() {
        return;
    }
    let boxed = Box::new(CompletionInner::new());
    unsafe {
        (*c).inner = Box::into_raw(boxed);
    }
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn destroy_completion(c: *mut CompletionShim) {
    if c.is_null() {
        return;
    }
    let inner = unsafe { (*c).inner };
    if !inner.is_null() {
        unsafe {
            drop(Box::from_raw(inner));
            (*c).inner = core::ptr::null_mut();
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn complete(c: *mut CompletionShim) {
    if let Some(inner) = deref_shim(c) {
        inner.flag.store(true, Ordering::Release);
        inner.wq.wake_one();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn complete_all(c: *mut CompletionShim) {
    if let Some(inner) = deref_shim(c) {
        inner.flag.store(true, Ordering::Release);
        inner.wq.wake_all();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn wait_for_completion(c: *mut CompletionShim) {
    let inner = match deref_shim(c) {
        Some(i) => i,
        None => return,
    };
    let _ = inner.wq.sleep_signalable_until(|| {
        if inner.flag.load(Ordering::Acquire) {
            Ok(Some(()))
        } else {
            Ok(None)
        }
    });
}

ksym!(init_completion);
ksym!(destroy_completion);
ksym!(complete);
ksym!(complete_all);
ksym!(wait_for_completion);
