// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Per-CPU kernel stack cache for fast fork/exit.
//!
//! Caches recently freed kernel stacks so fork can reuse them without
//! going through the buddy allocator. Reused stacks are warm in L1/L2
//! cache, eliminating the cache-cold penalty that dominates fork latency.

use crate::page_allocator::{alloc_pages_owned, AllocPageFlags, OwnedPages, PageAllocError};
use crate::spinlock::SpinLock;
use crate::arch::PAGE_SIZE;
use core::sync::atomic::{AtomicUsize, Ordering};

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
pub fn alloc_kernel_stack(num_pages: usize) -> Result<OwnedPages, PageAllocError> {
    let cache = match num_pages {
        4 => Some(&CACHE_4PAGE),
        2 => Some(&CACHE_2PAGE),
        _ => None,
    };

    if let Some(cache) = cache {
        if let Some(stack) = cache.lock_no_irq().pop() {
            install_guard(&stack);
            return Ok(stack);
        }
    }

    // Cache miss: allocate from buddy.
    let stack = alloc_pages_owned(num_pages, AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK)?;
    install_guard(&stack);
    Ok(stack)
}

// ── Stack Guard (Poison Pattern) ──────────────────────────────────────

/// Magic value written to the bottom 512 bytes of every kernel stack.
/// If any of these values are overwritten, a stack overflow occurred.
const GUARD_MAGIC: u64 = 0xDEAD_CAFE_DEAD_CAFE;
/// Number of u64 values in the guard region (512 bytes = 64 u64s).
const GUARD_WORDS: usize = 64;

/// Total number of stacks with guard patterns installed.
static GUARD_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Write the guard poison pattern at the bottom of a stack.
fn install_guard(stack: &OwnedPages) {
    #[allow(unsafe_code)]
    unsafe {
        let base = stack.as_vaddr().as_mut_ptr::<u64>();
        for i in 0..GUARD_WORDS {
            base.add(i).write_volatile(GUARD_MAGIC);
        }
    }
    GUARD_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Check the guard poison pattern at the bottom of a stack.
/// Returns `true` if the guard is intact, `false` if corrupted.
fn check_guard(stack: &OwnedPages) -> bool {
    #[allow(unsafe_code)]
    unsafe {
        let base = stack.as_vaddr().as_ptr::<u64>();
        for i in 0..GUARD_WORDS {
            if base.add(i).read_volatile() != GUARD_MAGIC {
                return false;
            }
        }
    }
    true
}

/// Check all cached stacks' guard patterns.  Called from `interval_work()`
/// periodically.  Panics if any guard is corrupted (stack overflow detected).
pub fn check_all_guards() {
    // Check cached 4-page stacks.
    {
        let cache = CACHE_4PAGE.lock_no_irq();
        for i in 0..cache.count {
            if let Some(ref stack) = cache.stacks[i] {
                if !check_guard(stack) {
                    panic!("STACK GUARD: kernel stack overflow detected in 4-page cached stack #{}", i);
                }
            }
        }
    }
    // Check cached 2-page stacks.
    {
        let cache = CACHE_2PAGE.lock_no_irq();
        for i in 0..cache.count {
            if let Some(ref stack) = cache.stacks[i] {
                if !check_guard(stack) {
                    panic!("STACK GUARD: kernel stack overflow detected in 2-page cached stack #{}", i);
                }
            }
        }
    }
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
