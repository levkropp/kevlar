// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};

use buddy_system_allocator::Heap;
use kevlar_utils::alignment::align_up;

use crate::arch::PAGE_SIZE;
use crate::page_allocator::{alloc_pages, AllocPageFlags};
use crate::spinlock::SpinLock;

const ORDER: usize = 32;
const KERNEL_HEAP_CHUNK_SIZE: usize = 4 * 1024 * 1024; // 4MiB

/// IRQ-safe wrapper around `buddy_system_allocator::Heap`.
///
/// The upstream `LockedHeapWithRescue` uses `spin::Mutex`, which does
/// NOT disable interrupts on acquire.  That is fine in a pure IF=0
/// kernel (no syscall ever runs with IF=1, so nothing can preempt the
/// allocator) but deadlocks the moment broad `sti` is applied in
/// `syscall_entry`: a user syscall holds the allocator lock, a timer
/// or virtio IRQ fires, the IRQ handler (deferred job / interval_work /
/// logger) calls `Vec::new()`, the global allocator tries to re-acquire
/// the same lock, and the IRQ spins forever while the holder is paused
/// in IRQ context.
///
/// Our `SpinLock::lock()` disables IF on acquire and restores on drop,
/// closing that window.
struct KevlarLockedHeap<const N: usize> {
    inner: SpinLock<Heap<N>>,
    rescue: fn(&mut Heap<N>, &Layout),
}

impl<const N: usize> KevlarLockedHeap<N> {
    const fn new(rescue: fn(&mut Heap<N>, &Layout)) -> Self {
        KevlarLockedHeap {
            inner: SpinLock::new(Heap::<N>::new()),
            rescue,
        }
    }

    /// Runtime access to the underlying heap — useful for the `rescue`
    /// path to seed the initial heap region before any allocation.
    pub fn lock(&self) -> kevlar_platform_proxy::SpinLockGuardProxy<'_, N> {
        kevlar_platform_proxy::SpinLockGuardProxy::new(self.inner.lock())
    }
}

// Helper module to forward the guard type without re-exporting the whole
// SpinLock API into this file's public surface.
mod kevlar_platform_proxy {
    use buddy_system_allocator::Heap;

    pub struct SpinLockGuardProxy<'a, const N: usize>(
        pub(super) crate::spinlock::SpinLockGuard<'a, Heap<N>>,
    );

    impl<'a, const N: usize> SpinLockGuardProxy<'a, N> {
        pub(super) fn new(g: crate::spinlock::SpinLockGuard<'a, Heap<N>>) -> Self {
            Self(g)
        }
    }

    impl<'a, const N: usize> core::ops::Deref for SpinLockGuardProxy<'a, N> {
        type Target = Heap<N>;
        fn deref(&self) -> &Heap<N> { &self.0 }
    }

    impl<'a, const N: usize> core::ops::DerefMut for SpinLockGuardProxy<'a, N> {
        fn deref_mut(&mut self) -> &mut Heap<N> { &mut self.0 }
    }
}

unsafe impl<const N: usize> GlobalAlloc for KevlarLockedHeap<N> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut inner = self.inner.lock();
        match inner.alloc(layout) {
            Ok(allocation) => allocation.as_ptr(),
            Err(_) => {
                (self.rescue)(&mut inner, &layout);
                inner
                    .alloc(layout)
                    .ok()
                    .map_or(core::ptr::null_mut(), |allocation| allocation.as_ptr())
            }
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe {
            self.inner.lock().dealloc(NonNull::new_unchecked(ptr), layout)
        }
    }
}

#[global_allocator]
static ALLOCATOR: KevlarLockedHeap<ORDER> = KevlarLockedHeap::new(expand_kernel_heap);
static KERNEL_HEAP_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn is_kernel_heap_enabled() -> bool {
    KERNEL_HEAP_ENABLED.load(Ordering::Acquire)
}

fn expand_kernel_heap(heap: &mut Heap<ORDER>, layout: &Layout) {
    if layout.size() > KERNEL_HEAP_CHUNK_SIZE {
        panic!(
            "tried to allocate too large object in the kernel heap (requested {} bytes)",
            layout.size()
        );
    }

    let num_pages = align_up(KERNEL_HEAP_CHUNK_SIZE, PAGE_SIZE) / PAGE_SIZE;
    let start = alloc_pages(num_pages, AllocPageFlags::KERNEL)
        .expect("run out of memory: failed to expand the kernel heap")
        .as_vaddr()
        .value();
    let end = start + KERNEL_HEAP_CHUNK_SIZE;

    unsafe {
        heap.add_to_heap(start, end);
    }

    KERNEL_HEAP_ENABLED.store(true, Ordering::Release)
}
