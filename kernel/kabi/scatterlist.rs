// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux scatter-gather list primitives.
//!
//! K12: stubs that satisfy the linker.  virtio_input.ko's
//! probe path uses sg_init_one to wrap a single buffer for
//! virtqueue_add_*; in K12 probe doesn't fire so this is
//! linker-only.

use core::ffi::c_void;

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn sg_init_one(
    _sg: *mut c_void,
    _buf: *const c_void,
    _len: u32,
) {
}

ksym!(sg_init_one);
