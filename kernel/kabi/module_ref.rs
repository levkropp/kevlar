// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux module refcount shim (`try_module_get` / `module_put`).
//!
//! Linux uses these to prevent unloading a module while a caller
//! holds a reference.  Kevlar doesn't unload modules yet, so:
//! `try_module_get` always succeeds; `module_put` is a no-op.

use core::ffi::c_void;

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn try_module_get(_module: *mut c_void) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn module_put(_module: *mut c_void) {}

ksym!(try_module_get);
ksym!(module_put);
