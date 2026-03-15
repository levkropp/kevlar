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
        REFCOUNTS[idx].fetch_add(1, Ordering::Relaxed);
    }
}

/// Decrement the reference count. Returns true if it dropped to 0
/// (caller should free the page).
#[inline]
pub fn page_ref_dec(paddr: PAddr) -> bool {
    let idx = pfn(paddr);
    if idx < MAX_PAGES {
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
