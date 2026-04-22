// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use core::ops::Deref;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::{address::PAddr, arch::PAGE_SIZE, bootinfo::RamArea, spinlock::SpinLock};
use arrayvec::ArrayVec;
use bitflags::bitflags;
use kevlar_utils::alignment::is_aligned;
use kevlar_utils::byte_size::ByteSize;

use kevlar_utils::buddy_alloc::BuddyAllocator as Allocator;

static ZONES: SpinLock<ArrayVec<Allocator, 8>> = SpinLock::new(ArrayVec::new_const());
static NUM_FREE_PAGES: AtomicUsize = AtomicUsize::new(0);
static NUM_TOTAL_PAGES: AtomicUsize = AtomicUsize::new(0);



/// A simple LIFO cache of single free pages to bypass the buddy allocator.
/// Sized to absorb large fault-around bursts (64 pages) across multiple
/// consecutive faults without immediate buddy allocator refills.
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

/// Pool of pre-zeroed 4KB pages. Served by `alloc_page()` when zeroed pages
/// are requested (!DIRTY_OK), avoiding the ~1-2µs inline memset.
/// Refilled at boot and in the idle thread.
const PREZEROED_4K_POOL_SIZE: usize = 128;

struct Prezeroed4kPool {
    pages: [usize; PREZEROED_4K_POOL_SIZE],
    count: usize,
}

impl Prezeroed4kPool {
    const fn new() -> Self {
        Prezeroed4kPool { pages: [0; PREZEROED_4K_POOL_SIZE], count: 0 }
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
        if self.count < PREZEROED_4K_POOL_SIZE {
            self.pages[self.count] = paddr.value();
            self.count += 1;
            true
        } else {
            false
        }
    }
}

static PREZEROED_4K_POOL: SpinLock<Prezeroed4kPool> = SpinLock::new(Prezeroed4kPool::new());
static PREZEROED_4K_COUNT: AtomicUsize = AtomicUsize::new(0);

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
        const KERNEL = 1 << 0;
        const USER = 1 << 1;
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

/// Refill the page cache from the buddy allocator in a single lock hold.
#[inline(never)]
fn refill_page_cache() -> usize {
    let mut buf = [0usize; PAGE_CACHE_SIZE];
    let mut count = 0;
    {
        let mut zones = ZONES.lock();
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
        let mut cache = PAGE_CACHE.lock();
        for i in 0..count {
            cache.push(PAddr::new(buf[i]));
        }
        PAGE_CACHE_COUNT.store(cache.count, Ordering::Relaxed);
        NUM_FREE_PAGES.fetch_sub(count, Ordering::Relaxed);
    }
    count
}

/// Allocate a single physical page.
#[inline(always)]
pub fn alloc_page(flags: AllocPageFlags) -> Result<PAddr, PageAllocError> {
    // Fastest path: pre-zeroed page — no memset needed.
    if !flags.contains(AllocPageFlags::DIRTY_OK) {
        if PREZEROED_4K_COUNT.load(Ordering::Relaxed) > 0 {
            if let Some(paddr) = PREZEROED_4K_POOL.lock().pop() {
                PREZEROED_4K_COUNT.fetch_sub(1, Ordering::Relaxed);
                check_not_stack(paddr);
                debug_assert_page_is_zero(paddr, "PREZEROED_POOL");
                return Ok(paddr);
            }
        }
    }

    // Fast path: pop from global page cache (lock_no_irq, ~5ns uncontended).
    if PAGE_CACHE_COUNT.load(Ordering::Relaxed) > 0 {
        let cached = PAGE_CACHE.lock().pop();
        if let Some(paddr) = cached {
            PAGE_CACHE_COUNT.fetch_sub(1, Ordering::Relaxed);
            check_not_stack(paddr);

            if !flags.contains(AllocPageFlags::DIRTY_OK) {
                unsafe { paddr.as_mut_ptr::<u8>().write_bytes(0, PAGE_SIZE); }
                debug_assert_page_is_zero(paddr, "PAGE_CACHE_memset");
            }
            NUM_FREE_PAGES.fetch_sub(1, Ordering::Relaxed);

            return Ok(paddr);
        }
    }

    // Slow path: refill cache from buddy allocator, then pop.
    if refill_page_cache() > 0 {
        let cached = PAGE_CACHE.lock().pop();
        if let Some(paddr) = cached {
            PAGE_CACHE_COUNT.fetch_sub(1, Ordering::Relaxed);
            check_not_stack(paddr);

            if !flags.contains(AllocPageFlags::DIRTY_OK) {
                unsafe { paddr.as_mut_ptr::<u8>().write_bytes(0, PAGE_SIZE); }
                debug_assert_page_is_zero(paddr, "PAGE_CACHE_slow_memset");
            }

            return Ok(paddr);
        }
    }

    Err(PageAllocError)
}

/// Runtime-enable flag for the zero-fill-on-alloc detector.  Cheap enough
/// (~1µs/alloc on 4KB page) to leave on during XFCE/LXDE/kernel tests but
/// we want the option to silence it in microbenchmarks.  Toggle via
/// `page_zero_check::set_enabled(false)`.  Default on.
static PAGE_ZERO_CHECK_ENABLED: AtomicBool = AtomicBool::new(true);

/// Count of zero-fill miss events.  Incremented regardless of whether the
/// sampled log fires (rate-limited below).  Dumpable via /proc helpers.
static PAGE_ZERO_MISS_COUNT: AtomicUsize = AtomicUsize::new(0);
static PAGE_ZERO_MISS_WITH_KERNEL_PTR: AtomicUsize = AtomicUsize::new(0);

/// Rate-limit log output: first N misses log full detail, rest silently
/// bump the counter.
const PAGE_ZERO_MISS_LOG_LIMIT: usize = 32;

pub fn page_zero_check_stats() -> (usize, usize) {
    (
        PAGE_ZERO_MISS_COUNT.load(Ordering::Relaxed),
        PAGE_ZERO_MISS_WITH_KERNEL_PTR.load(Ordering::Relaxed),
    )
}

pub fn set_page_zero_check_enabled(on: bool) {
    PAGE_ZERO_CHECK_ENABLED.store(on, Ordering::Relaxed);
}

/// Scan a freshly-zeroed page to verify it really is zero, and flag any
/// kernel-direct-map-shaped values we find.  This is the on-hand
/// instrumentation for task #25: a freshly-handed-to-user page that
/// still contains kernel data is the exact bug we're hunting.
///
/// Two passes in one loop:
/// 1. Any non-zero word → this page escaped memset somewhere
/// 2. Upper 17 bits = 0x1ffff → that word is a kernel direct-map VA
///    (the fault signature from blogs 186/187)
///
/// Cost: 512 volatile u64 loads (~1-2µs on a 4GHz core).  Acceptable
/// for the alloc-path because we already pay tens of µs here for
/// memset itself.
#[inline(always)]
fn debug_assert_page_is_zero(paddr: PAddr, site: &'static str) {
    if !PAGE_ZERO_CHECK_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    #[allow(unsafe_code)]
    unsafe {
        let ptr = paddr.as_ptr::<u64>();
        let mut first_hit: Option<usize> = None;
        let mut kernel_ptr_count = 0usize;
        let mut first_kernel_ptr: Option<(usize, u64)> = None;
        let mut nonzero_count = 0usize;
        for i in 0..(PAGE_SIZE / 8) {
            let v = core::ptr::read_volatile(ptr.add(i));
            if v != 0 {
                nonzero_count += 1;
                if first_hit.is_none() { first_hit = Some(i); }
                if (v >> 47) == 0x1ffff {
                    kernel_ptr_count += 1;
                    if first_kernel_ptr.is_none() {
                        first_kernel_ptr = Some((i, v));
                    }
                }
            }
        }
        if let Some(first) = first_hit {
            let n = PAGE_ZERO_MISS_COUNT.fetch_add(1, Ordering::Relaxed);
            if kernel_ptr_count > 0 {
                PAGE_ZERO_MISS_WITH_KERNEL_PTR.fetch_add(1, Ordering::Relaxed);
            }
            if n < PAGE_ZERO_MISS_LOG_LIMIT {
                log::warn!(
                    "PAGE_ZERO_MISS site={} paddr={:#x} first_nz_off={:#x} \
                     nonzero_words={} kernel_ptr_words={} (seen #{})",
                    site, paddr.value(), first * 8,
                    nonzero_count, kernel_ptr_count, n + 1,
                );
                if let Some((koff, kval)) = first_kernel_ptr {
                    log::warn!(
                        "    first kernel-VA word: paddr+{:#05x} = {:#018x} \
                         (target paddr={:#x})",
                        koff * 8, kval, kval & 0x0000_7fff_ffff_ffff,
                    );
                }
                // INSTRUMENTATION (task #25): correlate with recent
                // multi-page frees.  If this paddr is within a range that
                // was just freed as 4 pages (stack) or 2 pages (xsave),
                // report the allocation size + age.  Tells us whether
                // this paddr came from a recent stack-release path.
                if let Some((base, npages, _tsc)) = recent_multi_free_match(paddr) {
                    log::warn!(
                        "    MULTI_FREE_MATCH: within recent {}-page free \
                         starting at paddr={:#x} (offset_into_alloc={:#x})",
                        npages, base, paddr.value() - base,
                    );
                }
                if let Some(_tsc) = recent_single_free_match(paddr) {
                    log::warn!(
                        "    SINGLE_FREE_MATCH: paddr={:#x} freed as single \
                         page within the last {} frees",
                        paddr.value(), SINGLE_FREE_RING_SIZE,
                    );
                }
                // Dump up to 16 kernel-VA words scattered through the page.
                // This shows the full leak pattern (first + last offsets,
                // target paddr distribution) so we can identify the
                // underlying data structure.
                if kernel_ptr_count > 0 {
                    let mut dumped = 0;
                    let mut min_off = usize::MAX;
                    let mut max_off = 0;
                    let mut min_target = u64::MAX;
                    let mut max_target = 0u64;
                    for i in 0..(PAGE_SIZE / 8) {
                        let v = core::ptr::read_volatile(ptr.add(i));
                        if (v >> 47) == 0x1ffff {
                            let target = v & 0x0000_7fff_ffff_ffff;
                            if i * 8 < min_off { min_off = i * 8; }
                            if i * 8 > max_off { max_off = i * 8; }
                            if target < min_target { min_target = target; }
                            if target > max_target { max_target = target; }
                            if dumped < 16 {
                                log::warn!(
                                    "    KVA[+{:#05x}] = {:#018x}  (target paddr={:#x})",
                                    i * 8, v, target,
                                );
                                dumped += 1;
                            }
                        }
                    }
                    log::warn!(
                        "    KVA_SUMMARY: {} kernel-VA words in page, \
                         offsets [{:#x}..{:#x}], target paddrs [{:#x}..{:#x}]",
                        kernel_ptr_count, min_off, max_off, min_target, max_target,
                    );
                }
                // Dump a 64-byte window around the first non-zero word.
                let start = first.saturating_sub(4);
                let end = core::cmp::min(first + 8, PAGE_SIZE / 8);
                for i in start..end {
                    let v = core::ptr::read_volatile(ptr.add(i));
                    let mark = if i == first { " <<<" } else { "" };
                    log::warn!("    +{:#05x}: {:#018x}{}", i * 8, v, mark);
                }
            }
        }
    }
}

/// Smoking-gun check: panic if the buddy/cache returned a paddr that
/// the stack_cache has registered as a live kernel stack — that's the
/// hypothesized refill_page_cache crash root cause.
#[inline]
fn check_not_stack(paddr: PAddr) {
    if crate::stack_cache::is_stack_paddr(paddr) {
        panic!("BUDDY double-alloc: paddr={:#x} is a live kernel stack", paddr.value());
    }
}

/// Pop a single pre-zeroed 4KB page from the pool, or None if empty.
#[inline(always)]
pub fn alloc_page_prezeroed() -> Option<PAddr> {
    if PREZEROED_4K_COUNT.load(Ordering::Relaxed) > 0 {
        let result = PREZEROED_4K_POOL.lock().pop();
        if result.is_some() {
            PREZEROED_4K_COUNT.fetch_sub(1, Ordering::Relaxed);
        }
        result
    } else {
        None
    }
}

/// Current count of pre-zeroed 4KB pages available.
#[inline(always)]
pub fn prezeroed_4k_count() -> usize {
    PREZEROED_4K_COUNT.load(Ordering::Relaxed)
}

/// Batch-allocate up to `max` dirty pages into `out`. Returns number allocated.
#[inline(always)]
pub fn alloc_page_batch(out: &mut [PAddr], max: usize) -> usize {
    let max = if max < out.len() { max } else { out.len() };
    let mut count = 0;

    // Drain from cache first.
    if PAGE_CACHE_COUNT.load(Ordering::Relaxed) > 0 {
        let mut cache = PAGE_CACHE.lock();
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

    // Allocate remaining from buddy allocator directly.
    if count < max {
        let mut zones = ZONES.lock();
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
    if num_pages == 1 {
        return alloc_page(flags);
    }

    let order = num_pages_to_order(num_pages);
    let mut zones = ZONES.lock();
    for zone in zones.iter_mut() {
        if let Some(paddr) = zone.alloc_pages(order) {
            let paddr = PAddr::new(paddr);
            drop(zones);

            // Smoking-gun check: if buddy returned a paddr that's still
            // registered as a kernel stack, we have the double-allocation
            // bug. Any zero-fill via the DIRTY_OK=false path below would
            // wipe the live stack.
            for i in 0..num_pages {
                let p = PAddr::new(paddr.value() + i * PAGE_SIZE);
                if crate::stack_cache::is_stack_paddr(p) {
                    panic!("BUDDY double-alloc: paddr={:#x} is a live kernel stack",
                        p.value());
                }
            }

            // INSTRUMENTATION (task #25): also check whether the paddr is
            // currently queued in PAGE_CACHE or PREZEROED_4K_POOL.  If so,
            // buddy is about to hand the same paddr to a multi-page
            // caller (us) while it's also queued for a single-page
            // consumer — that's the page-in-two-pools bug.
            for i in 0..num_pages {
                let p = PAddr::new(paddr.value() + i * PAGE_SIZE);
                let target = p.value();
                let cache = PAGE_CACHE.lock();
                let in_cache = cache.pages[..cache.count].iter().any(|&x| x == target);
                drop(cache);
                if in_cache {
                    panic!("BUDDY_POOL_OVERLAP: paddr={:#x} handed by zone \
                           while in PAGE_CACHE (multi-page alloc of {} from {:#x})",
                        p.value(), num_pages, paddr.value());
                }
                let prezeroed = PREZEROED_4K_POOL.lock();
                let in_prezeroed = prezeroed.pages[..prezeroed.count].iter().any(|&x| x == target);
                drop(prezeroed);
                if in_prezeroed {
                    panic!("BUDDY_POOL_OVERLAP: paddr={:#x} handed by zone \
                           while in PREZEROED_POOL (multi-page alloc of {} from {:#x})",
                        p.value(), num_pages, paddr.value());
                }
            }

            if !flags.contains(AllocPageFlags::DIRTY_OK) {
                unsafe {
                    paddr.as_mut_ptr::<u8>().write_bytes(0, num_pages * PAGE_SIZE);
                }
            }
            NUM_FREE_PAGES.fetch_sub(num_pages, Ordering::Relaxed);
            return Ok(paddr);
        }
    }

    Err(PageAllocError)
}

/// Allocate a 2MB-aligned huge page (512 contiguous 4KB pages).
/// The buddy allocator order-9 guarantees 2MB alignment.
/// Returns DIRTY memory — caller must zero if needed.
pub fn alloc_huge_page() -> Result<PAddr, PageAllocError> {
    alloc_pages(512, AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK)
}

/// Pool of pre-zeroed 2MB huge pages.  Zeroing happens at free time
/// (munmap), so the next fault gets a pre-zeroed page at zero cost.
const PREZEROED_HUGE_POOL_SIZE: usize = 8;

struct HugePagePool {
    pages: [usize; PREZEROED_HUGE_POOL_SIZE],
    count: usize,
}

impl HugePagePool {
    const fn new() -> Self {
        HugePagePool {
            pages: [0; PREZEROED_HUGE_POOL_SIZE],
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
        if self.count < PREZEROED_HUGE_POOL_SIZE {
            self.pages[self.count] = paddr.value();
            self.count += 1;
            true
        } else {
            false
        }
    }
}

static PREZEROED_HUGE_POOL: SpinLock<HugePagePool> = SpinLock::new(HugePagePool::new());

/// Try to allocate a pre-zeroed 2MB huge page from the pool.
/// Returns `None` if the pool is empty — caller should fall back to
/// `alloc_huge_page()` + `zero_huge_page()`.
#[inline]
pub fn alloc_huge_page_prezeroed() -> Option<PAddr> {
    PREZEROED_HUGE_POOL.lock().pop()
}

/// Zero a 2MB huge page and add it to the pre-zeroed pool.
/// If the pool is full, the page is freed back to the buddy allocator.
/// Called from munmap when a sole-owner huge page is unmapped.
pub fn free_huge_page_and_zero(paddr: PAddr) {
    use crate::page_ops::zero_huge_page;
    zero_huge_page(paddr);
    if !PREZEROED_HUGE_POOL.lock().push(paddr) {
        // Pool full — return to buddy allocator.
        free_pages(paddr, 512);
    } else {
        NUM_FREE_PAGES.fetch_add(512, Ordering::Relaxed);
    }
}

/// Pre-fill the prezeroed huge page pool at boot time so the first
/// userspace 2MB faults get instant pre-zeroed pages without paying
/// the alloc+zero cost on the hot path.
pub fn prefill_huge_page_pool() {
    for _ in 0..PREZEROED_HUGE_POOL_SIZE {
        match alloc_huge_page() {
            Ok(paddr) => free_huge_page_and_zero(paddr),
            Err(_) => break,
        }
    }
}

/// Pre-fill the 4KB prezeroed page pool at boot. Pages are allocated
/// from the buddy allocator, zeroed, and placed in the pool so the
/// first page faults get instant zeroed pages without inline memset.
pub fn prefill_prezeroed_pages() {
    use crate::page_ops::zero_page;
    let target = PREZEROED_4K_POOL_SIZE;
    for _ in 0..target {
        match alloc_page(AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK) {
            Ok(paddr) => {
                zero_page(paddr);
                // INSTRUMENTATION (task #25): verify the page is actually
                // zero right after zero_page().  If it isn't, buddy handed
                // us a page that's still being written to — either the
                // allocator's double-free detector is broken, or some code
                // path is freeing a page that's still live.
                debug_assert_page_is_zero(paddr, "PREZEROED_PREPUSH");
                let mut pool = PREZEROED_4K_POOL.lock();
                if !pool.push(paddr) {
                    drop(pool);
                    free_pages(paddr, 1);
                    break;
                }
                PREZEROED_4K_COUNT.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => break,
        }
    }
}

/// Refill the 4KB prezeroed pool from the buddy allocator.
/// Called from the idle thread to keep the pool warm between bursts.
/// Returns the number of pages added.
pub fn refill_prezeroed_pages() -> usize {
    use crate::page_ops::zero_page;
    let current = PREZEROED_4K_COUNT.load(Ordering::Relaxed);
    if current >= PREZEROED_4K_POOL_SIZE / 2 {
        return 0; // Pool is at least half full, don't refill.
    }

    let target = PREZEROED_4K_POOL_SIZE - current;
    let mut filled = 0;
    for _ in 0..target {
        match alloc_page(AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK) {
            Ok(paddr) => {
                zero_page(paddr);
                // Same INSTRUMENTATION as prefill.  refill() is called
                // periodically from the idle thread, so a corruption here
                // would point at a live kernel write to a supposedly-freed
                // paddr — exactly the task #25 signature.
                debug_assert_page_is_zero(paddr, "PREZEROED_PREPUSH_REFILL");
                let mut pool = PREZEROED_4K_POOL.lock();
                if pool.push(paddr) {
                    PREZEROED_4K_COUNT.fetch_add(1, Ordering::Relaxed);
                    filled += 1;
                } else {
                    drop(pool);
                    free_pages(paddr, 1);
                    break;
                }
            }
            Err(_) => break,
        }
    }
    filled
}

pub fn alloc_pages_owned(
    num_pages: usize,
    flags: AllocPageFlags,
) -> Result<OwnedPages, PageAllocError> {
    alloc_pages(num_pages, flags).map(|paddr| OwnedPages::new(paddr, num_pages))
}

/// Returns true if this physical address belongs to a managed memory zone.
/// Device memory (PCI BARs, framebuffers) returns false.
pub fn is_managed_page(paddr: PAddr) -> bool {
    let mut zones = ZONES.lock();
    for zone in zones.iter_mut() {
        if zone.includes(paddr.value()) {
            return true;
        }
    }
    false
}

/// Ring buffer of recent free_pages calls (paddr, num_pages, tsc).
/// On PAGE_ZERO_MISS we can correlate: was this paddr just freed?  The
/// single-page ring answers "did free_pages(paddr, 1) happen recently?"
/// (→ CoW path / mmap teardown).  The multi-page ring answers "was this
/// paddr part of a stack/xsave release?"  (→ task-drop path).  Distinct
/// rings because single-page frees fire 10-100x more often than multi.
const MULTI_FREE_RING_SIZE: usize = 32;
static MULTI_FREE_RING: SpinLock<[(usize, usize, u64); MULTI_FREE_RING_SIZE]> =
    SpinLock::new([(0, 0, 0); MULTI_FREE_RING_SIZE]);
static MULTI_FREE_IDX: AtomicUsize = AtomicUsize::new(0);

const SINGLE_FREE_RING_SIZE: usize = 128;
static SINGLE_FREE_RING: SpinLock<[(usize, u64); SINGLE_FREE_RING_SIZE]> =
    SpinLock::new([(0, 0); SINGLE_FREE_RING_SIZE]);
static SINGLE_FREE_IDX: AtomicUsize = AtomicUsize::new(0);

/// Check if `paddr` is within a recent multi-page free (last 32 frees).
/// Returns (num_pages, tsc) of the matching free, or None.
pub fn recent_multi_free_match(paddr: PAddr) -> Option<(usize, usize, u64)> {
    let ring = MULTI_FREE_RING.lock();
    let target = paddr.value();
    for &(base, num, tsc) in ring.iter() {
        if num > 1 && target >= base && target < base + num * PAGE_SIZE {
            return Some((base, num, tsc));
        }
    }
    None
}

/// Check if `paddr` was recently freed as a single page (last 128 frees).
/// Returns the TSC of the matching free, or None.
pub fn recent_single_free_match(paddr: PAddr) -> Option<u64> {
    let ring = SINGLE_FREE_RING.lock();
    let target = paddr.value();
    for &(p, tsc) in ring.iter() {
        if p == target {
            return Some(tsc);
        }
    }
    None
}

pub fn free_pages(paddr: PAddr, num_pages: usize) {

    // INSTRUMENTATION (task #25): panic if we're about to return a paddr
    // to the allocator while it's still registered as an active kernel
    // stack.  The `is_stack_paddr` registry is populated by
    // `alloc_kernel_stack` and cleared by `free_kernel_stack` BEFORE
    // the OwnedPages is handed to OwnedPages::drop -> free_pages — so a
    // positive hit here proves some OTHER path is calling free_pages
    // on a live stack (the exact live-page-to-buddy bug we're hunting).
    if num_pages > 1 {
        for i in 0..num_pages {
            let p = PAddr::new(paddr.value() + i * PAGE_SIZE);
            if crate::stack_cache::is_stack_paddr(p) {
                panic!(
                    "FREE_LIVE_STACK: paddr={:#x} (within multi-page free of \
                     {} starting at {:#x}) is a live kernel stack!",
                    p.value(), num_pages, paddr.value(),
                );
            }
        }

        // Record the multi-page free in the ring buffer so that a later
        // PAGE_ZERO_MISS can correlate: did this paddr come from a
        // multi-page allocation that just got freed?
        #[cfg(target_arch = "x86_64")]
        {
            let tsc = unsafe { core::arch::x86_64::_rdtsc() };
            let idx = MULTI_FREE_IDX.fetch_add(1, Ordering::Relaxed) % MULTI_FREE_RING_SIZE;
            let mut ring = MULTI_FREE_RING.lock();
            ring[idx] = (paddr.value(), num_pages, tsc);
        }
    }

    // INSTRUMENTATION (task #25): AP kernel stacks are never freed.
    // Panic if anyone tries to free (any size) into an AP-stack range.
    // Cheap check: AP_STACK_BASES is MAX_CPUS atomics.
    #[cfg(target_arch = "x86_64")]
    {
        for i in 0..num_pages {
            let p = PAddr::new(paddr.value() + i * PAGE_SIZE);
            if crate::x64::smp::is_ap_stack_paddr(p) {
                panic!(
                    "FREE_AP_STACK: paddr={:#x} (within free of {} pages from \
                     {:#x}) is a live AP kernel stack!",
                    p.value(), num_pages, paddr.value(),
                );
            }
        }
    }

    // Single page — try to push to cache.
    if num_pages == 1 {
        // INSTRUMENTATION (task #25): record the free in the single-page
        // ring so PAGE_ZERO_MISS can correlate.  Cheap: one lock + one
        // store.  Ring size (128) keeps ~50 µs of recent frees under
        // typical XFCE load.
        #[cfg(target_arch = "x86_64")]
        {
            let tsc = unsafe { core::arch::x86_64::_rdtsc() };
            let idx = SINGLE_FREE_IDX.fetch_add(1, Ordering::Relaxed) % SINGLE_FREE_RING_SIZE;
            let mut ring = SINGLE_FREE_RING.lock();
            ring[idx] = (paddr.value(), tsc);
        }
        if PAGE_CACHE_COUNT.load(Ordering::Relaxed) < PAGE_CACHE_SIZE {
            let mut cache = PAGE_CACHE.lock();
            if cache.push(paddr) {
                PAGE_CACHE_COUNT.fetch_add(1, Ordering::Relaxed);
                NUM_FREE_PAGES.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
    }

    let order = num_pages_to_order(num_pages);
    let mut zones = ZONES.lock();
    for zone in zones.iter_mut() {
        if zone.includes(paddr.value()) {
            zone.free_pages(paddr.value(), order);
            NUM_FREE_PAGES.fetch_add(num_pages, Ordering::Relaxed);
            return;
        }
    }

    panic!("invalid page address: {:?}", paddr);
}

/// Pre-warm KVM EPT entries by touching physical pages through the straight-map.
///
/// Under KVM, the first access to a guest physical page triggers an EPT
/// violation (~13µs).  Subsequent accesses to the same page only need a
/// TLB refill (~200ns).  By pre-touching pages at boot, we ensure that
/// the benchmark's page fault zeroing path hits warm EPT entries.
///
/// Allocates and frees `count` order-9 blocks (each 2MB).  With buddy
/// coalescing, the freed blocks merge back to order-9 and are available
/// for alloc_huge_page with warm EPT.
pub fn pre_warm_ept(count: usize) {
    use crate::address::PAddr;

    for _ in 0..count {
        let block = match alloc_pages(512, AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK) {
            Ok(p) => p,
            Err(_) => break,
        };
        // Write to each 4KB page within the 2MB block to create EPT
        // entries with WRITE permission.  A read would only create a
        // read-only EPT entry, requiring a second VM exit to upgrade
        // when the page is later zeroed.
        for i in 0..512 {
            let page = PAddr::new(block.value() + i * PAGE_SIZE);
            unsafe {
                core::ptr::write_volatile(page.as_mut_ptr::<u8>(), 0);
            }
        }
        // Free back — coalescing merges it to order-9 with warm EPT.
        free_pages(block, 512);
    }
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
