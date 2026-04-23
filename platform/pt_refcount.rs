// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Page-table page reference counting for lazy CoW fork.
//!
//! Separate from `page_refcount` (data pages) because a PT page's
//! lifecycle and the semantics of its refcount are distinct: PT pages
//! are shared between process address spaces when `PTE_SHARED_PT` is set
//! on a pointing descriptor, and get unshared (copy-up) on the first
//! write through any mutation site that walks the shared table.
//!
//! PT pages start at refcount 1 when allocated (via `alloc_pt_page`).
//! Fork increments by 1 for each shared PT.  Teardown decrements.  When
//! refcount hits 0, the PT page is freed.
//!
//! At any moment, a physical page is either a data page (tracked in
//! `page_refcount`) or a PT page (tracked here) — but never both.  The
//! two arrays coexist safely because an ordinary callsite knows which
//! kind it's holding.

use core::sync::atomic::{AtomicU16, Ordering};
use crate::address::PAddr;
use crate::arch::PAGE_SIZE;

/// Maximum supported physical memory (PT pages are a subset of all
/// physical pages).  Matches `page_refcount::MAX_PAGES`.
/// 1M pages × 2 bytes = 2 MiB static.
const MAX_PAGES: usize = 1024 * 1024;

static PT_REFCOUNTS: [AtomicU16; MAX_PAGES] = {
    const ZERO: AtomicU16 = AtomicU16::new(0);
    [ZERO; MAX_PAGES]
};

#[inline(always)]
fn pfn(paddr: PAddr) -> usize {
    paddr.value() / PAGE_SIZE
}

/// Set refcount to 1 at PT page allocation time.
#[inline]
pub fn pt_ref_init(paddr: PAddr) {
    let idx = pfn(paddr);
    if idx < MAX_PAGES {
        PT_REFCOUNTS[idx].store(1, Ordering::Relaxed);
    }
}

/// Bump a PT page's refcount; called when fork shares the PT.
#[inline]
pub fn pt_ref_inc(paddr: PAddr) {
    let idx = pfn(paddr);
    if idx < MAX_PAGES {
        PT_REFCOUNTS[idx].fetch_add(1, Ordering::Relaxed);
    }
}

/// Decrement a PT page's refcount.  Returns `true` if the count dropped
/// to 0 (caller should free the page via `free_pages`).
#[inline]
pub fn pt_ref_dec(paddr: PAddr) -> bool {
    let idx = pfn(paddr);
    if idx < MAX_PAGES {
        let prev = PT_REFCOUNTS[idx].fetch_sub(1, Ordering::Relaxed);
        debug_assert!(prev > 0, "pt_refcount underflow at {:#x}", paddr.value());
        prev == 1
    } else {
        true // unknown page — let caller free it
    }
}

/// Read the current refcount without modifying it.
#[inline]
pub fn pt_ref_count(paddr: PAddr) -> u16 {
    let idx = pfn(paddr);
    if idx < MAX_PAGES {
        PT_REFCOUNTS[idx].load(Ordering::Relaxed)
    } else {
        1
    }
}
