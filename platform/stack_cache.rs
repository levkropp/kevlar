// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Per-CPU kernel stack cache for fast fork/exit.
//!
//! Caches recently freed kernel stacks so fork can reuse them without
//! going through the buddy allocator. Reused stacks are warm in L1/L2
//! cache, eliminating the cache-cold penalty that dominates fork latency.

use crate::page_allocator::{alloc_pages_owned, AllocPageFlags, OwnedPages, PageAllocError};
use crate::spinlock::SpinLock;
use crate::address::PAddr;
use crate::arch::PAGE_SIZE;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Registry of physical pages currently allocated as kernel stacks.
/// `is_stack_paddr` consults this to detect the double-allocation bug
/// where the buddy allocator returns a paddr that's still in use as
/// a stack — the smoking gun for the XFCE refill_page_cache crash.
const STACK_REGISTRY_SIZE: usize = 256;
static STACK_REGISTRY: SpinLock<[u64; STACK_REGISTRY_SIZE]> =
    SpinLock::new([0u64; STACK_REGISTRY_SIZE]);

fn register_stack(paddr: PAddr, num_pages: usize) {
    let mut reg = STACK_REGISTRY.lock();
    let base = paddr.value() as u64;
    for i in 0..num_pages {
        let p = base + (i * PAGE_SIZE) as u64;
        // Find a free slot.
        for slot in reg.iter_mut() {
            if *slot == 0 {
                *slot = p;
                break;
            }
        }
    }
}

fn unregister_stack(paddr: PAddr, num_pages: usize) {
    let mut reg = STACK_REGISTRY.lock();
    let base = paddr.value() as u64;
    for i in 0..num_pages {
        let p = base + (i * PAGE_SIZE) as u64;
        for slot in reg.iter_mut() {
            if *slot == p {
                *slot = 0;
                break;
            }
        }
    }
}

/// Check whether a paddr is currently registered as a kernel stack.
/// Used by alloc_pages / zero_page to detect double-allocation.
pub fn is_stack_paddr(paddr: PAddr) -> bool {
    let reg = STACK_REGISTRY.lock();
    let p = paddr.value() as u64;
    reg.iter().any(|&s| s == p)
}

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
        if let Some(stack) = cache.lock().pop() {
            install_guard(&stack);
            register_stack(*stack, num_pages);
            return Ok(stack);
        }
    }

    // Cache miss: allocate from buddy.
    let stack = alloc_pages_owned(num_pages, AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK)?;
    install_guard(&stack);
    register_stack(*stack, num_pages);
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

/// Scan every live (currently allocated) kernel stack for the
/// "top byte zeroed" corruption signature. Live stacks are tracked in
/// `STACK_REGISTRY`. Reading a stack while its owning task runs on
/// another CPU is safe: the pattern check is for an exact non-canonical
/// value (`(v >> 32) == 0x00ff_8000`) that cannot appear as a transient
/// intermediate of an aligned 8-byte mov, and aligned writes are atomic
/// on x86_64 (Intel SDM 8.1.1). False positives would require legitimate
/// kernel code to write that exact value, which doesn't happen.
///
/// Logs (does not panic) on hit, so we can correlate detector output
/// with subsequent crashes.
/// Hashed (paddr, offset, value) triples already reported, to suppress
/// the same hit firing every interval. Linear-probe table; collisions
/// just yield extra reports (acceptable).
const REPORT_DEDUPE_SIZE: usize = 64;
static REPORTED_HITS: SpinLock<[u64; REPORT_DEDUPE_SIZE]> =
    SpinLock::new([0u64; REPORT_DEDUPE_SIZE]);

fn dedupe_check_and_record(paddr: u64, offset: usize, value: u64) -> bool {
    // Cheap mix of (paddr, offset, value) into a non-zero u64 key.
    let key = paddr.wrapping_mul(0x9e3779b97f4a7c15)
        ^ ((offset as u64).wrapping_mul(0xbf58476d1ce4e5b9))
        ^ value.wrapping_mul(0x94d049bb133111eb);
    let key = if key == 0 { 1 } else { key };
    let mut tbl = REPORTED_HITS.lock();
    let idx = (key as usize) % REPORT_DEDUPE_SIZE;
    for i in 0..REPORT_DEDUPE_SIZE {
        let slot = (idx + i) % REPORT_DEDUPE_SIZE;
        if tbl[slot] == key {
            return false; // already reported
        }
        if tbl[slot] == 0 {
            tbl[slot] = key;
            return true;
        }
    }
    false // table full, suppress
}

pub fn scan_live_stack_corruption() {
    let snapshot: [u64; STACK_REGISTRY_SIZE] = {
        let reg = STACK_REGISTRY.lock();
        *reg
    };
    let self_rsp: u64;
    #[allow(unsafe_code)]
    unsafe {
        core::arch::asm!("mov {}, rsp", out(reg) self_rsp, options(nomem, nostack));
    }
    let self_page_va = self_rsp & !0xfff;
    for &p in snapshot.iter() {
        if p == 0 {
            continue;
        }
        let paddr = PAddr::new(p as usize);
        let page_va = paddr.as_vaddr().value() as u64;
        if page_va == self_page_va {
            continue;
        }
        let base = paddr.as_ptr::<u64>();
        #[allow(unsafe_code)]
        unsafe {
            for i in 0..(PAGE_SIZE / 8) {
                let v = base.add(i).read_volatile();
                if (v >> 32) == 0x00ff_8000 {
                    if !dedupe_check_and_record(p, i * 8, v) {
                        return;
                    }
                    log::warn!(
                        "LIVE STACK CORRUPT: paddr={:#x} offset={:#x} \
                         value={:#x}",
                        p, i * 8, v,
                    );
                    // Dump 8 qwords before and after for context.
                    let lo = if i >= 8 { i - 8 } else { 0 };
                    let hi = if i + 8 < PAGE_SIZE / 8 { i + 8 } else { PAGE_SIZE / 8 - 1 };
                    for j in lo..=hi {
                        let marker = if j == i { " <==" } else { "" };
                        log::warn!(
                            "  [{:+#06x}] = {:#018x}{}",
                            (j as isize - i as isize) * 8,
                            base.add(j).read_volatile(),
                            marker,
                        );
                    }
                    return;
                }
            }
        }
    }
}

/// Check all cached stacks' guard patterns.  Called from `interval_work()`
/// periodically.  Panics if any guard is corrupted (stack overflow detected).
pub fn check_all_guards() {
    // Check cached 4-page stacks.
    {
        let cache = CACHE_4PAGE.lock();
        for i in 0..cache.count {
            if let Some(ref stack) = cache.stacks[i] {
                if !check_guard(stack) {
                    panic!("STACK GUARD: kernel stack overflow detected in 4-page cached stack #{}", i);
                }
                scan_corruption_pattern(stack, KERNEL_STACK_SIZE_4P, "4-page-cache", i);
            }
        }
    }
    // Check cached 2-page stacks.
    {
        let cache = CACHE_2PAGE.lock();
        for i in 0..cache.count {
            if let Some(ref stack) = cache.stacks[i] {
                if !check_guard(stack) {
                    panic!("STACK GUARD: kernel stack overflow detected in 2-page cached stack #{}", i);
                }
                scan_corruption_pattern(stack, KERNEL_STACK_SIZE_2P, "2-page-cache", i);
            }
        }
    }
}

const KERNEL_STACK_SIZE_4P: usize = 4 * PAGE_SIZE;
const KERNEL_STACK_SIZE_2P: usize = 2 * PAGE_SIZE;

/// Scan a stack for the "top byte zeroed" corruption pattern that masks the
/// XFCE crash. A valid kernel pointer on the stack has the form
/// 0xffff_8000_xxxx_xxxx; if a stale TLB write or other corruption clears
/// the top byte, we'd see 0x00ff_8000_xxxx_xxxx — non-canonical, never
/// produced by normal kernel code. Catching this in a CACHED stack proves
/// the corruption happened to a freed-and-recycled page.
fn scan_corruption_pattern(stack: &OwnedPages, size: usize, kind: &str, idx: usize) {
    #[allow(unsafe_code)]
    unsafe {
        let base = stack.as_vaddr().as_ptr::<u64>();
        let words = size / 8;
        for i in 0..words {
            let v = base.add(i).read_volatile();
            // The corruption signature: top byte 0x00, second-top byte 0xff,
            // then the canonical kernel-VA prefix 0x80_00. Equivalent to the
            // upper 32 bits being exactly 0x00ff_8000.
            if (v >> 32) == 0x00ff_8000 {
                let stack_base = stack.as_vaddr().value();
                log::warn!(
                    "STACK CORRUPT DETECTED: kind={} idx={} stack_base={:#x} \
                     offset={:#x} value={:#x}",
                    kind, idx, stack_base, i * 8, v,
                );
                panic!("STACK CORRUPT: top byte zeroed at offset {:#x}, val={:#x}",
                    i * 8, v);
            }
        }
    }
}

/// Return a kernel stack to the cache. If the cache is full, the stack
/// is dropped (freed via buddy allocator through OwnedPages::drop).
pub fn free_kernel_stack(stack: OwnedPages, num_pages: usize) {
    // Unregister BEFORE handing back, so the registry doesn't
    // erroneously claim a freed page is still a stack.
    unregister_stack(*stack, num_pages);

    let cache = match num_pages {
        4 => Some(&CACHE_4PAGE),
        2 => Some(&CACHE_2PAGE),
        _ => None,
    };

    if let Some(cache) = cache {
        let mut c = cache.lock();
        match c.push(stack) {
            Ok(()) => return, // cached successfully
            Err(_stack) => return, // cache full, _stack dropped here → freed via buddy
        }
    }
    // Unsupported size: drop(stack) implicit — frees via buddy
}
