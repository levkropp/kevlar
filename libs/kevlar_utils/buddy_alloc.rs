// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Buddy allocator with coalescing on free and allocation bitmap.
//!
//! Uses intrusive free lists stored in the free pages themselves.
//! On free, coalesces with the buddy block if both are free, recursing
//! up to MAX_ORDER.
//!
//! An allocation bitmap tracks which pages are currently allocated.
//! `free_coalesce` only merges with a buddy whose bitmap bits are ALL
//! clear, preventing coalescing with pages that left the buddy's free
//! lists (e.g., pages sitting in the page-allocator's PAGE_CACHE).

const PAGE_SIZE: usize = 4096;

/// Maximum block order: 2^MAX_ORDER pages = 4MB.
const MAX_ORDER: usize = 10;

/// Maximum pages tracked by the global bitmap.
///
/// Bitmap is indexed by absolute paddr / PAGE_SIZE, so the range must cover
/// every paddr the allocator manages — starting from paddr 0, not the zone
/// base.  ARM64 QEMU virt places RAM at 0x40000000 (1 GiB), so to cover RAM
/// at all we need MAX ≥ 262144.  To support up to 16 GiB of RAM on any arch,
/// we size the bitmap at 4M pages (512 KB static).  Out-of-range indices
/// early-return from the bitmap helpers below; an allocator attempting to
/// free a paddr past this range used to trip the "double free" assert in
/// `free_coalesce` because `bitmap_is_allocated` silently returned false —
/// the bigger bitmap closes that asymmetric-behaviour bug.
const MAX_BITMAP_PAGES: usize = 4 * 1024 * 1024;
const BITMAP_BYTES: usize = MAX_BITMAP_PAGES / 8;

/// Global allocation bitmap shared by all zones.
/// Indexed by physical page number (paddr / PAGE_SIZE).
/// bit=1 → page is allocated, bit=0 → page is free.
///
/// Safety: only accessed while the ZONES SpinLock is held (same lock
/// that protects BuddyAllocator), so no additional synchronization needed.
static mut ALLOC_BITMAP: [u8; BITMAP_BYTES] = [0u8; BITMAP_BYTES];

#[inline(always)]
fn bitmap_is_allocated(paddr: usize) -> bool {
    let idx = paddr / PAGE_SIZE;
    if idx >= MAX_BITMAP_PAGES { return false; }
    unsafe { (ALLOC_BITMAP[idx / 8] & (1 << (idx % 8))) != 0 }
}

#[inline(always)]
fn bitmap_set_allocated(paddr: usize) {
    let idx = paddr / PAGE_SIZE;
    if idx >= MAX_BITMAP_PAGES { return; }
    unsafe { ALLOC_BITMAP[idx / 8] |= 1 << (idx % 8); }
}

#[inline(always)]
fn bitmap_clear_allocated(paddr: usize) {
    let idx = paddr / PAGE_SIZE;
    if idx >= MAX_BITMAP_PAGES { return; }
    unsafe { ALLOC_BITMAP[idx / 8] &= !(1 << (idx % 8)); }
}

pub struct BuddyAllocator {
    free_lists: [usize; MAX_ORDER + 1], // head paddr per order (0 = empty)
    base: usize,     // start of managed region (paddr)
    end: usize,      // end of managed region (paddr)
    total_pages: usize,
    /// Offset to convert physical address → kernel virtual address.
    vaddr_offset: usize,
}

// Safety: BuddyAllocator is only accessed under a SpinLock in page_allocator.rs.
unsafe impl Send for BuddyAllocator {}

impl BuddyAllocator {
    /// Create a new buddy allocator over the given physical memory region.
    ///
    /// # Safety
    ///
    /// `base` must be a valid kernel virtual address for the start of the
    /// memory region (straight-mapped).  `base_paddr` is the physical
    /// address.  `len` is the total byte length (must be page-aligned).
    pub unsafe fn new(base: *mut u8, base_paddr: usize, len: usize) -> BuddyAllocator {
        let num_pages = len / PAGE_SIZE;
        let vaddr_offset = (base as usize).wrapping_sub(base_paddr);

        let mut alloc = BuddyAllocator {
            free_lists: [0; MAX_ORDER + 1],
            base: base_paddr,
            end: base_paddr + num_pages * PAGE_SIZE,
            total_pages: num_pages,
            vaddr_offset,
        };

        // Build free lists by creating the largest aligned blocks possible.
        // All pages start as "free" in the bitmap (global static is zero-init).
        let mut offset = 0; // in pages
        while offset < num_pages {
            let paddr = base_paddr + offset * PAGE_SIZE;
            let mut order = MAX_ORDER;
            loop {
                let block_pages = 1usize << order;
                // Block must fit AND be naturally aligned to its size.
                if block_pages <= (num_pages - offset)
                    && (paddr % (block_pages * PAGE_SIZE)) == 0
                {
                    break;
                }
                if order == 0 {
                    break;
                }
                order -= 1;
            }
            alloc.push_free(paddr, order);
            offset += 1usize << order;
        }

        alloc
    }

    pub fn num_total_pages(&self) -> usize {
        self.total_pages
    }

    pub fn includes(&mut self, ptr: usize) -> bool {
        self.base <= ptr && ptr < self.end
    }

    /// Allocate a single page.  O(1) when order-0 list is non-empty.
    #[inline(always)]
    pub fn alloc_one(&mut self) -> Option<usize> {
        self.alloc_order(0)
    }

    /// Free a single page with coalescing.
    #[inline(always)]
    pub fn free_one(&mut self, ptr: usize) {
        debug_assert!(ptr >= self.base && ptr < self.end);
        debug_assert!(ptr % PAGE_SIZE == 0);
        self.free_coalesce(ptr, 0);
    }

    /// Allocate 2^order contiguous pages.
    pub fn alloc_pages(&mut self, order: usize) -> Option<usize> {
        self.alloc_order(order)
    }

    /// Free 2^order contiguous pages with coalescing.
    pub fn free_pages(&mut self, ptr: usize, order: usize) {
        debug_assert!(ptr >= self.base && ptr < self.end);
        self.free_coalesce(ptr, order);
    }

    /// Pop a block from the given order's free list, splitting higher-order
    /// blocks if needed.  Marks all pages as allocated in the bitmap.
    #[inline(always)]
    fn alloc_order(&mut self, target_order: usize) -> Option<usize> {
        // Fast path: pop from target order.
        if self.free_lists[target_order] != 0 {
            let block = self.pop_free(target_order);
            let count = 1usize << target_order;
            for i in 0..count {
                let page = block + i * PAGE_SIZE;
                // ALWAYS-on assertion (task #25): if a page in the free
                // list is marked allocated in the bitmap, the buddy's
                // state is corrupted — usually because free_pages was
                // called on a still-allocated paddr, double-queueing it
                // into the free lists.  Panic so we get a crash dump
                // instead of silently re-handing out a live paddr.
                assert!(!bitmap_is_allocated(page),
                    "buddy_alloc: double alloc page {:#x} block {:#x} order {}",
                    page, block, target_order);
                bitmap_set_allocated(page);
            }
            return Some(block);
        }

        // Find the smallest non-empty higher-order list and split down.
        let mut order = target_order + 1;
        while order <= MAX_ORDER {
            if self.free_lists[order] != 0 {
                break;
            }
            order += 1;
        }
        if order > MAX_ORDER {
            return None;
        }

        // Split from `order` down to `target_order`.
        let mut block = self.pop_free(order);
        while order > target_order {
            order -= 1;
            // Put the second half (buddy) on the lower-order free list.
            let buddy = block + ((1usize << order) * PAGE_SIZE);
            self.push_free(buddy, order);
        }

        let count = 1usize << target_order;
        for i in 0..count {
            let page = block + i * PAGE_SIZE;
            assert!(!bitmap_is_allocated(page),
                "buddy_alloc: double alloc (split) page {:#x} block {:#x} order {}",
                page, block, target_order);
            bitmap_set_allocated(page);
        }

        Some(block)
    }

    /// Free a block with buddy coalescing.
    /// Only coalesces with a buddy if ALL its pages are free in the bitmap.
    fn free_coalesce(&mut self, mut ptr: usize, mut order: usize) {
        let count = 1usize << order;
        for i in 0..count {
            let page = ptr + i * PAGE_SIZE;
            // ALWAYS-on assertion (task #25): freeing a page that isn't
            // currently marked allocated means free_pages is being called
            // twice on the same paddr (or called on a paddr that was
            // never allocated).  Either way the buddy state is corrupt.
            assert!(bitmap_is_allocated(page),
                "buddy_alloc: double free page {:#x} block {:#x} order {}",
                page, ptr, order);
            bitmap_clear_allocated(page);
        }

        while order < MAX_ORDER {
            let buddy = ptr ^ ((1usize << order) * PAGE_SIZE);
            // Buddy must be within our managed region.
            if buddy < self.base || buddy >= self.end {
                break;
            }
            // Bitmap guard: only coalesce if ALL buddy pages are free.
            // This prevents merging with pages that are in the PAGE_CACHE
            // (removed from buddy free lists but still "allocated").
            let buddy_pages = 1usize << order;
            if (0..buddy_pages).any(|i| bitmap_is_allocated(buddy + i * PAGE_SIZE)) {
                break;
            }
            // Check if buddy is in the free list at this order.
            if self.remove_from_free_list(buddy, order) {
                // Merge: combined block starts at the lower address.
                ptr = if ptr < buddy { ptr } else { buddy };
                order += 1;
            } else {
                break;
            }
        }
        self.push_free(ptr, order);
    }

    /// Try to remove a specific block from an order's free list.
    /// Returns true if found and removed, false otherwise.
    fn remove_from_free_list(&mut self, target: usize, order: usize) -> bool {
        let mut prev_ptr: Option<usize> = None;
        let mut current = self.free_lists[order];

        while current != 0 {
            if current == target {
                // Found it.  Read next pointer from the block.
                let vaddr = current.wrapping_add(self.vaddr_offset);
                let next = unsafe { *(vaddr as *const usize) };
                match prev_ptr {
                    Some(prev) => {
                        let prev_vaddr = prev.wrapping_add(self.vaddr_offset);
                        unsafe { *(prev_vaddr as *mut usize) = next; }
                    }
                    None => {
                        self.free_lists[order] = next;
                    }
                }
                return true;
            }
            prev_ptr = Some(current);
            let vaddr = current.wrapping_add(self.vaddr_offset);
            current = unsafe { *(vaddr as *const usize) };
        }

        false
    }

    /// Push a block onto the given order's free list.
    #[inline(always)]
    fn push_free(&mut self, paddr: usize, order: usize) {
        let vaddr = paddr.wrapping_add(self.vaddr_offset);
        unsafe {
            *(vaddr as *mut usize) = self.free_lists[order];
        }
        self.free_lists[order] = paddr;
    }

    /// Pop a block from the given order's free list.
    #[inline(always)]
    fn pop_free(&mut self, order: usize) -> usize {
        let block = self.free_lists[order];
        debug_assert!(block != 0, "pop_free on empty list");
        let vaddr = block.wrapping_add(self.vaddr_offset);
        let next = unsafe { *(vaddr as *const usize) };
        self.free_lists[order] = next;
        block
    }
}
