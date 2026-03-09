// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Safe wrappers for physical page operations.

use crate::address::PAddr;
use crate::arch::PAGE_SIZE;

/// Zero-fill a physical page.
pub fn zero_page(paddr: PAddr) {
    unsafe {
        paddr.as_mut_ptr::<u8>().write_bytes(0, PAGE_SIZE);
    }
}

/// Get a mutable byte slice over a physical page.
///
/// # Safety guarantee
/// The returned slice is valid as long as the page is mapped in the
/// kernel's straight-map region (which all allocated pages are).
pub fn page_as_slice_mut(paddr: PAddr) -> &'static mut [u8] {
    unsafe { core::slice::from_raw_parts_mut(paddr.as_mut_ptr(), PAGE_SIZE) }
}
