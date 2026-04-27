// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! `param_ops_int` and friends — Linux module-parameter `kernel_param_ops`
//! singleton structs that modules embed in their own `module_param`
//! tables.
//!
//! Linux exports these as static instances: a module that does
//! `module_param(numdummies, int, 0)` records `&param_ops_int` in its
//! `__param` section.  The kernel later calls `param_ops_int.set()`
//! to parse a sysfs write into the parameter's storage.  K11: stub
//! set/get to no-ops returning 0; default value already in the
//! module's .data section is what the module sees.

use core::ffi::{c_char, c_void};

use crate::ksym_static;

#[repr(C)]
pub struct KernelParamOps {
    pub flags: u32,
    pub set: Option<extern "C" fn(*const c_char, *const c_void) -> i32>,
    pub get: Option<extern "C" fn(*mut c_char, *const c_void) -> i32>,
    pub free: Option<extern "C" fn(*mut c_void)>,
}

extern "C" fn param_set_int_stub(_val: *const c_char, _kp: *const c_void) -> i32 {
    0
}

extern "C" fn param_get_int_stub(_buf: *mut c_char, _kp: *const c_void) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub static param_ops_int: KernelParamOps = KernelParamOps {
    flags: 0,
    set: Some(param_set_int_stub),
    get: Some(param_get_int_stub),
    free: None,
};

ksym_static!(param_ops_int);
