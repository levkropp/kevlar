// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! DMA reservation object (`dma_resv`) shim.
//!
//! Linux's `dma_resv` manages fence-based synchronization between
//! CPU and GPU access to buffers.  K14 only needs the symbol resolved
//! at link time — drm_exec doesn't fire it at load.

use core::ffi::c_void;

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn dma_resv_reserve_fences(
    _obj: *mut c_void,
    _num_fences: u32,
) -> i32 {
    0
}

ksym!(dma_resv_reserve_fences);
