// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Simple buddy allocator with O(1) single-page allocation.
//!
//! Uses intrusive free lists stored in the free pages themselves (zero
//! external metadata overhead).  No coalescing on free — freed blocks go
//! back to their order's free list without merging.  This is optimal for
//! our workload where nearly all allocs are single-page (demand paging)
//! and multi-page allocs happen at boot and are rarely freed.

const PAGE_SIZE: usize = 4096;

/// Maximum block order: 2^MAX_ORDER pages = 4MB.
const MAX_ORDER: usize = 10;

pub struct BuddyAllocator {
    free_lists: [usize; MAX_ORDER + 1], // head paddr per order (0 = empty)
    base: usize,     // start of managed region (paddr)
    end: usize,      // end of managed region (paddr)
    total_pages: usize,
    /// Offset to convert physical address → kernel virtual address.
    /// Computed as `(base_vaddr - base_paddr)`.  All page accesses use
    /// `(paddr + vaddr_offset) as *mut usize` for the straight-map.
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

    /// Free a single page.  O(1), no coalescing.
    #[inline(always)]
    pub fn free_one(&mut self, ptr: usize) {
        debug_assert!(ptr >= self.base && ptr < self.end);
        debug_assert!(ptr % PAGE_SIZE == 0);
        self.push_free(ptr, 0);
    }

    /// Allocate 2^order contiguous pages.
    pub fn alloc_pages(&mut self, order: usize) -> Option<usize> {
        self.alloc_order(order)
    }

    /// Free 2^order contiguous pages.  No coalescing.
    pub fn free_pages(&mut self, ptr: usize, order: usize) {
        debug_assert!(ptr >= self.base && ptr < self.end);
        self.push_free(ptr, order);
    }

    /// Pop a block from the given order's free list, splitting higher-order
    /// blocks if needed.
    #[inline(always)]
    fn alloc_order(&mut self, target_order: usize) -> Option<usize> {
        // Fast path: pop from target order.
        if self.free_lists[target_order] != 0 {
            return Some(self.pop_free(target_order));
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

        Some(block)
    }

    /// Push a block onto the given order's free list.
    /// Writes the next-pointer into the block's first 8 bytes via straight-map.
    #[inline(always)]
    fn push_free(&mut self, paddr: usize, order: usize) {
        let vaddr = paddr.wrapping_add(self.vaddr_offset);
        unsafe {
            *(vaddr as *mut usize) = self.free_lists[order];
        }
        self.free_lists[order] = paddr;
    }

    /// Pop a block from the given order's free list.
    /// Reads the next-pointer from the block's first 8 bytes via straight-map.
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
