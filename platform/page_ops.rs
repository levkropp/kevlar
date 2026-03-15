// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Safe wrappers for physical page operations.

use crate::address::PAddr;
use crate::arch::PAGE_SIZE;

/// Zero-fill a physical page using `rep stosq` for hardware-optimized bulk fill.
#[inline(always)]
pub fn zero_page(paddr: PAddr) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let ptr = paddr.as_mut_ptr::<u64>();
        core::arch::asm!(
            "rep stosq",
            inout("rdi") ptr => _,
            inout("rcx") (PAGE_SIZE / 8) => _,
            in("rax") 0u64,
            options(nostack),
        );
    }
    #[cfg(not(target_arch = "x86_64"))]
    unsafe {
        paddr.as_mut_ptr::<u8>().write_bytes(0, PAGE_SIZE);
    }
}

/// Zero-fill a 2MB huge page in 4KB chunks.
///
/// Under KVM, EPT entries for each 4KB page within a contiguous 2MB
/// allocation are cold (buddy alloc only touches page 0).  Zeroing in
/// 4KB chunks keeps each `rep stosq` within L1 cache, and the EPT
/// violation per page is handled once — the CPU resumes `rep stosq`
/// after KVM installs the entry.
#[inline(always)]
pub fn zero_huge_page(paddr: PAddr) {
    for i in 0..512 {
        zero_page(PAddr::new(paddr.value() + i * PAGE_SIZE));
    }
}

/// Get a mutable byte slice over a physical page.
///
/// # Safety guarantee
/// The returned slice is valid as long as the page is mapped in the
/// kernel's straight-map region (which all allocated pages are).
///
/// Not available under the Fortress profile — use [`PageFrame`] instead.
/// Read-only view of a physical page as a byte slice.
#[cfg(not(feature = "profile-fortress"))]
pub fn page_as_slice(paddr: PAddr) -> &'static [u8] {
    unsafe { core::slice::from_raw_parts(paddr.as_ptr(), PAGE_SIZE) }
}

#[cfg(not(feature = "profile-fortress"))]
pub fn page_as_slice_mut(paddr: PAddr) -> &'static mut [u8] {
    unsafe { core::slice::from_raw_parts_mut(paddr.as_mut_ptr(), PAGE_SIZE) }
}

/// Copy-semantic access to a physical page frame.
///
/// Instead of handing out a raw `&mut [u8]` into physical memory, this type
/// exposes `read` and `write` methods that copy data through a caller-owned
/// buffer. This prevents aliased references to page memory from leaking
/// across ring boundaries.
pub struct PageFrame {
    paddr: PAddr,
}

impl PageFrame {
    pub fn new(paddr: PAddr) -> Self {
        PageFrame { paddr }
    }

    /// Copy bytes out of the page at `offset` into `dst`.
    pub fn read(&self, offset: usize, dst: &mut [u8]) {
        assert!(offset + dst.len() <= PAGE_SIZE);
        unsafe {
            core::ptr::copy_nonoverlapping(
                self.paddr.as_ptr::<u8>().add(offset),
                dst.as_mut_ptr(),
                dst.len(),
            );
        }
    }

    /// Copy bytes from `src` into the page at `offset`.
    pub fn write(&mut self, offset: usize, src: &[u8]) {
        assert!(offset + src.len() <= PAGE_SIZE);
        unsafe {
            core::ptr::copy_nonoverlapping(
                src.as_ptr(),
                self.paddr.as_mut_ptr::<u8>().add(offset),
                src.len(),
            );
        }
    }
}
