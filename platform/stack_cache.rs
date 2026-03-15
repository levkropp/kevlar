// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Per-CPU kernel stack cache for fast fork/exit.
//!
//! Caches recently freed kernel stacks so fork can reuse them without
//! going through the buddy allocator. Reused stacks are warm in L1/L2
//! cache, eliminating the cache-cold penalty that dominates fork latency.

use crate::page_allocator::{alloc_pages_owned, AllocPageFlags, OwnedPages};
use crate::spinlock::SpinLock;

/// Number of cached stacks per size class.
const CACHE_SIZE: usize = 4;

/// A cache of recently freed stacks of a specific page count.
struct SizeCache {
    stacks: [Option<OwnedPages>; CACHE_SIZE],
    count: usize,
}

impl SizeCache {
    const fn new() -> Self {
        SizeCache {
            stacks: [const { None }; CACHE_SIZE],
            count: 0,
        }
    }

    fn pop(&mut self) -> Option<OwnedPages> {
        if self.count == 0 {
            return None;
        }
        self.count -= 1;
        self.stacks[self.count].take()
    }

    fn push(&mut self, stack: OwnedPages) -> Result<(), OwnedPages> {
        if self.count >= CACHE_SIZE {
            return Err(stack); // cache full, return ownership to caller
        }
        self.stacks[self.count] = Some(stack);
        self.count += 1;
        Ok(())
    }
}

// Stack caches for common sizes: 4-page (kernel) and 2-page (IST).
static CACHE_4PAGE: SpinLock<SizeCache> = SpinLock::new(SizeCache::new());
static CACHE_2PAGE: SpinLock<SizeCache> = SpinLock::new(SizeCache::new());

/// Allocate a kernel stack, preferring the cache over the buddy allocator.
/// Cached stacks are warm in L1/L2 cache from recent use.
pub fn alloc_kernel_stack(num_pages: usize) -> OwnedPages {
    let cache = match num_pages {
        4 => Some(&CACHE_4PAGE),
        2 => Some(&CACHE_2PAGE),
        _ => None,
    };

    if let Some(cache) = cache {
        if let Some(stack) = cache.lock_no_irq().pop() {
            return stack;
        }
    }

    // Cache miss: allocate from buddy.
    alloc_pages_owned(num_pages, AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK)
        .expect("failed to allocate kernel stack")
}

/// Return a kernel stack to the cache. If the cache is full, the stack
/// is dropped (freed via buddy allocator through OwnedPages::drop).
pub fn free_kernel_stack(stack: OwnedPages, num_pages: usize) {
    let cache = match num_pages {
        4 => Some(&CACHE_4PAGE),
        2 => Some(&CACHE_2PAGE),
        _ => None,
    };

    if let Some(cache) = cache {
        let mut c = cache.lock_no_irq();
        match c.push(stack) {
            Ok(()) => return, // cached successfully
            Err(_stack) => return, // cache full, _stack dropped here → freed via buddy
        }
    }
    // Unsupported size: drop(stack) implicit — frees via buddy
}
