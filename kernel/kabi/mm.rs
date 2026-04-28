// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux MM (memory-management) helpers exposed via kABI.
//!
//! - `is_vmalloc_addr`: real implementation — walks K2's
//!   VMALLOC_TABLE in `kernel/kabi/alloc.rs` (re-exported via
//!   `vmalloc_addr_lookup`) and returns whether the pointer
//!   came from `vmalloc` / `vzalloc`.
//! - `__vma_start_write`: mm-internal hook for VMA write
//!   tracking; no-op at K16.
//! - `vm_get_page_prot`: K16 returns 0 (Linux's
//!   `pgprot_t` for PROT_NONE).  K22+ when real VMAs are set
//!   up against module memory we revisit.
//! - `memstart_addr`: aarch64 direct-map base.  K16 exposes 0;
//!   no caller dereferences yet.

use core::ffi::c_void;

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn is_vmalloc_addr(addr: *const c_void) -> bool {
    super::alloc::is_vmalloc_addr_internal(addr as usize)
}

#[unsafe(no_mangle)]
pub extern "C" fn __vma_start_write(_vma: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn vm_get_page_prot(_vm_flags: usize) -> u64 {
    0
}

ksym!(is_vmalloc_addr);
ksym!(__vma_start_write);
ksym!(vm_get_page_prot);

// `memstart_addr` is the aarch64 direct-map base.  K16: expose 0;
// callers that fire (K17+) will surface a need for a real value.
#[unsafe(no_mangle)]
pub static memstart_addr: u64 = 0;
crate::ksym_static!(memstart_addr);
