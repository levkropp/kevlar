// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Physical page reference counting for Copy-on-Write (CoW) fork.
//!
//! Each physical page frame has a u16 reference count. Pages start at
//! refcount 1 when allocated. Fork increments to 2 (shared between
//! parent and child). Write faults allocate a new page and decrement
//! the old one. When refcount reaches 0, the page is freed.

use core::sync::atomic::{AtomicU16, Ordering};
use crate::arch::PAGE_SIZE;
use crate::address::PAddr;

/// Maximum supported physical memory: 4 GiB = 1M pages.
/// Each AtomicU16 = 2 bytes → 2 MiB refcount array.
const MAX_PAGES: usize = 1024 * 1024;

/// Sentinel refcount for kernel-image pages (e.g. initramfs data mapped
/// directly into user space). These pages must never be freed.
pub const PAGE_REF_KERNEL_IMAGE: u16 = u16::MAX;

static REFCOUNTS: [AtomicU16; MAX_PAGES] = {
    // const initializer for the array
    const ZERO: AtomicU16 = AtomicU16::new(0);
    [ZERO; MAX_PAGES]
};

#[inline(always)]
fn pfn(paddr: PAddr) -> usize {
    paddr.value() / PAGE_SIZE
}

/// Increment the reference count for a physical page.
/// Called when a CoW fork shares a page between parent and child.
#[inline]
pub fn page_ref_inc(paddr: PAddr) {
    let idx = pfn(paddr);
    if idx < MAX_PAGES {
        if REFCOUNTS[idx].load(Ordering::Relaxed) == PAGE_REF_KERNEL_IMAGE {
            return; // Sentinel: kernel-image page, never modify.
        }
        REFCOUNTS[idx].fetch_add(1, Ordering::Relaxed);
    }
}

/// Decrement the reference count. Returns true if it dropped to 0
/// (caller should free the page).
#[inline]
pub fn page_ref_dec(paddr: PAddr) -> bool {
    let idx = pfn(paddr);
    if idx < MAX_PAGES {
        if REFCOUNTS[idx].load(Ordering::Relaxed) == PAGE_REF_KERNEL_IMAGE {
            return false; // Sentinel: kernel-image page, never free.
        }
        let prev = REFCOUNTS[idx].fetch_sub(1, Ordering::Relaxed);
        debug_assert!(prev > 0, "page refcount underflow at {:#x}", paddr.value());
        prev == 1
    } else {
        true // unknown page, let caller free it
    }
}

/// Get the current reference count for a page.
#[inline]
pub fn page_ref_count(paddr: PAddr) -> u16 {
    let idx = pfn(paddr);
    if idx < MAX_PAGES {
        REFCOUNTS[idx].load(Ordering::Relaxed)
    } else {
        1
    }
}

/// Set the reference count to 1 (for freshly allocated pages).
#[inline]
pub fn page_ref_init(paddr: PAddr) {
    let idx = pfn(paddr);
    if idx < MAX_PAGES {
        REFCOUNTS[idx].store(1, Ordering::Relaxed);
    }
}

/// Set the sentinel refcount for a kernel-image page.
/// These pages are part of the kernel binary and must never be freed.
#[inline]
pub fn page_ref_init_kernel_image(paddr: PAddr) {
    let idx = pfn(paddr);
    if idx < MAX_PAGES {
        REFCOUNTS[idx].store(PAGE_REF_KERNEL_IMAGE, Ordering::Relaxed);
    }
}

/// Initialize refcounts for all 512 sub-pages of a 2MB huge page.
/// Each sub-PFN gets refcount 1.
///
/// Uses `rep stosw` for hardware-optimized bulk fill instead of 512
/// individual atomic stores.  Safe because these pages are not yet
/// mapped — no concurrent reader exists.  On x86_64 TSO, the
/// subsequent PDE write is observed in order after these stores.
pub fn page_ref_init_huge(base: PAddr) {
    let base_idx = pfn(base);
    debug_assert!(base_idx + 512 <= MAX_PAGES);
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let ptr = REFCOUNTS[base_idx].as_ptr();
        core::arch::asm!(
            "rep stosw",
            inout("rdi") ptr => _,
            inout("rcx") 512usize => _,
            in("ax") 1u16,
            options(nostack),
        );
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        for i in 0..512 {
            let idx = base_idx + i;
            if idx < MAX_PAGES {
                REFCOUNTS[idx].store(1, Ordering::Relaxed);
            }
        }
    }
}

/// Increment refcounts for all 512 sub-pages of a 2MB huge page.
pub fn page_ref_inc_huge(base: PAddr) {
    let base_idx = pfn(base);
    for i in 0..512 {
        let idx = base_idx + i;
        if idx < MAX_PAGES {
            REFCOUNTS[idx].fetch_add(1, Ordering::Relaxed);
        }
    }
}
