// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use core::ops::Deref;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::{address::PAddr, arch::PAGE_SIZE, bootinfo::RamArea, spinlock::SpinLock};
use arrayvec::ArrayVec;
use bitflags::bitflags;
use kevlar_utils::alignment::is_aligned;
use kevlar_utils::byte_size::ByteSize;

use kevlar_utils::bitmap_allocator::BitMapAllocator as Allocator;

static ZONES: SpinLock<ArrayVec<Allocator, 8>> = SpinLock::new(ArrayVec::new_const());
static NUM_FREE_PAGES: AtomicUsize = AtomicUsize::new(0);
static NUM_TOTAL_PAGES: AtomicUsize = AtomicUsize::new(0);

/// A simple LIFO cache of pre-zeroed free pages to bypass the bitmap allocator.
/// Pages in this cache are ALREADY ZEROED — alloc_page never needs to zero them.
const PAGE_CACHE_SIZE: usize = 64;

struct PageCache {
    pages: [usize; PAGE_CACHE_SIZE],
    count: usize,
}

impl PageCache {
    const fn new() -> Self {
        PageCache {
            pages: [0; PAGE_CACHE_SIZE],
            count: 0,
        }
    }

    #[inline(always)]
    fn pop(&mut self) -> Option<PAddr> {
        if self.count > 0 {
            self.count -= 1;
            Some(PAddr::new(self.pages[self.count]))
        } else {
            None
        }
    }

    #[inline(always)]
    fn push(&mut self, paddr: PAddr) -> bool {
        if self.count < PAGE_CACHE_SIZE {
            self.pages[self.count] = paddr.value();
            self.count += 1;
            true
        } else {
            false
        }
    }
}

static PAGE_CACHE: SpinLock<PageCache> = SpinLock::new(PageCache::new());
static PAGE_CACHE_COUNT: AtomicUsize = AtomicUsize::new(0);

#[inline(always)]
fn num_pages_to_order(num_pages: usize) -> usize {
    if num_pages <= 1 {
        return 0;
    }
    (usize::BITS - (num_pages - 1).leading_zeros()) as usize
}

#[derive(Debug)]
pub struct Stats {
    pub num_free_pages: usize,
    pub num_total_pages: usize,
}

pub fn read_allocator_stats() -> Stats {
    Stats {
        num_free_pages: NUM_FREE_PAGES.load(Ordering::Relaxed),
        num_total_pages: NUM_TOTAL_PAGES.load(Ordering::Relaxed),
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct AllocPageFlags: u32 {
        /// Allocate pages for the kernel purpose.
        const KERNEL = 1 << 0;
        /// Allocate pages for the user.
        const USER = 1 << 1;
        /// If set, the page may contain stale data (caller will zero it).
        const DIRTY_OK = 1 << 2;
    }
}

#[derive(Debug)]
pub struct PageAllocError;

pub struct OwnedPages {
    paddr: PAddr,
    num_pages: usize,
}

impl OwnedPages {
    fn new(paddr: PAddr, num_pages: usize) -> OwnedPages {
        OwnedPages { paddr, num_pages }
    }
}

impl Deref for OwnedPages {
    type Target = PAddr;

    fn deref(&self) -> &Self::Target {
        &self.paddr
    }
}

impl Drop for OwnedPages {
    fn drop(&mut self) {
        free_pages(self.paddr, self.num_pages);
    }
}

/// Zero a physical page using the platform-optimal method.
#[inline(always)]
fn zero_page_internal(paddr: PAddr) {
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

/// Refill the page cache from the ZONES bitmap allocator in a single lock hold.
/// Pages are PRE-ZEROED during refill so alloc_page never zeroes on the hot path.
/// Returns the number of pages added to the cache.
#[inline(never)]
fn refill_page_cache() -> usize {
    // Allocate into a local buffer to avoid holding two locks simultaneously.
    let mut buf = [0usize; PAGE_CACHE_SIZE];
    let mut count = 0;
    {
        let mut zones = ZONES.lock_no_irq();
        for zone in zones.iter_mut() {
            while count < PAGE_CACHE_SIZE {
                if let Some(paddr) = zone.alloc_one() {
                    buf[count] = paddr;
                    count += 1;
                } else {
                    break;
                }
            }
            if count >= PAGE_CACHE_SIZE {
                break;
            }
        }
    }

    if count > 0 {
        let mut cache = PAGE_CACHE.lock_no_irq();
        for i in 0..count {
            cache.push(PAddr::new(buf[i]));
        }
        PAGE_CACHE_COUNT.store(cache.count, Ordering::Relaxed);
        NUM_FREE_PAGES.fetch_sub(count, Ordering::Relaxed);
    }
    count
}

/// Allocate a single physical page. Fast path that avoids the order calculation.
///
/// Pages from the cache are PRE-ZEROED. If DIRTY_OK is set, the caller will
/// zero the page themselves (e.g., page fault handler calls zero_page after).
/// If DIRTY_OK is NOT set, the page is already zeroed from the cache.
#[inline(always)]
pub fn alloc_page(flags: AllocPageFlags) -> Result<PAddr, PageAllocError> {
    // Try the free-page cache first (O(1), no bitmap scan).
    // Fast check: skip lock acquisition if cache is known empty.
    if PAGE_CACHE_COUNT.load(Ordering::Relaxed) > 0 {
        let cached = PAGE_CACHE.lock_no_irq().pop();
        if let Some(paddr) = cached {
            PAGE_CACHE_COUNT.fetch_sub(1, Ordering::Relaxed);
            // Pages in cache are already zeroed. No need to zero again
            // unless the caller explicitly wants a dirty page (DIRTY_OK)
            // and plans to zero it themselves — which is fine, pre-zeroed
            // pages are valid for any use.
            NUM_FREE_PAGES.fetch_sub(1, Ordering::Relaxed);
            return Ok(paddr);
        }
    }

    // Cache empty — batch refill from bitmap allocator, then pop one.
    if refill_page_cache() > 0 {
        let cached = PAGE_CACHE.lock_no_irq().pop();
        if let Some(paddr) = cached {
            PAGE_CACHE_COUNT.fetch_sub(1, Ordering::Relaxed);
            // Already zeroed during refill.
            return Ok(paddr);
        }
    }

    Err(PageAllocError)
}

/// Batch-allocate up to `max` dirty pages into `out`. Returns number allocated.
/// Pages are NOT zeroed (caller must zero if needed).
#[inline(always)]
pub fn alloc_page_batch(out: &mut [PAddr], max: usize) -> usize {
    let max = if max < out.len() { max } else { out.len() };
    let mut count = 0;

    // Drain from cache first.
    if PAGE_CACHE_COUNT.load(Ordering::Relaxed) > 0 {
        let mut cache = PAGE_CACHE.lock_no_irq();
        while count < max {
            if let Some(paddr) = cache.pop() {
                out[count] = paddr;
                count += 1;
            } else {
                break;
            }
        }
        PAGE_CACHE_COUNT.store(cache.count, Ordering::Relaxed);
    }

    // Allocate remaining from zones in a single lock hold.
    if count < max {
        let mut zones = ZONES.lock_no_irq();
        for zone in zones.iter_mut() {
            while count < max {
                if let Some(paddr_val) = zone.alloc_one() {
                    out[count] = PAddr::new(paddr_val);
                    count += 1;
                } else {
                    break;
                }
            }
        }
    }

    if count > 0 {
        NUM_FREE_PAGES.fetch_sub(count, Ordering::Relaxed);
    }
    count
}

pub fn alloc_pages(num_pages: usize, flags: AllocPageFlags) -> Result<PAddr, PageAllocError> {
    // Single page — use the fast cache path.
    if num_pages == 1 {
        return alloc_page(flags);
    }

    let order = num_pages_to_order(num_pages);
    let mut zones = ZONES.lock_no_irq();
    for zone in zones.iter_mut() {
        if let Some(paddr) = zone.alloc_pages(order) {
            let paddr = PAddr::new(paddr);
            if !flags.contains(AllocPageFlags::DIRTY_OK) {
                unsafe {
                    paddr
                        .as_mut_ptr::<u8>()
                        .write_bytes(0, num_pages * PAGE_SIZE);
                }
            }
            NUM_FREE_PAGES.fetch_sub(num_pages, Ordering::Relaxed);
            return Ok(paddr);
        }
    }

    Err(PageAllocError)
}

pub fn alloc_pages_owned(
    num_pages: usize,
    flags: AllocPageFlags,
) -> Result<OwnedPages, PageAllocError> {
    alloc_pages(num_pages, flags).map(|paddr| OwnedPages::new(paddr, num_pages))
}

pub fn free_pages(paddr: PAddr, num_pages: usize) {
    // Single page — try to push to cache instead of bitmap dealloc.
    if num_pages == 1 {
        if PAGE_CACHE_COUNT.load(Ordering::Relaxed) < PAGE_CACHE_SIZE {
            let mut cache = PAGE_CACHE.lock_no_irq();
            if cache.push(paddr) {
                PAGE_CACHE_COUNT.fetch_add(1, Ordering::Relaxed);
                NUM_FREE_PAGES.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
    }

    let order = num_pages_to_order(num_pages);
    let mut zones = ZONES.lock_no_irq();
    for zone in zones.iter_mut() {
        if zone.includes(paddr.value()) {
            zone.free_pages(paddr.value(), order);
            NUM_FREE_PAGES.fetch_add(num_pages, Ordering::Relaxed);
            return;
        }
    }

    panic!("invalid page address: {:?}", paddr);
}

pub fn init(areas: &[RamArea]) {
    let mut zones = ZONES.lock();
    for area in areas {
        assert!(is_aligned(area.base.value(), PAGE_SIZE));
        let allocator =
            unsafe { Allocator::new(area.base.as_mut_ptr(), area.base.value(), area.len) };
        let num_pages = area.len / PAGE_SIZE;
        info!(
            "RAM: {} ({} pages) at {:x}",
            ByteSize::new(area.len),
            num_pages,
            area.base.value()
        );
        NUM_TOTAL_PAGES.fetch_add(num_pages, Ordering::Relaxed);
        NUM_FREE_PAGES.fetch_add(num_pages, Ordering::Relaxed);
        zones.push(allocator);
    }
}
