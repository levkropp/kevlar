// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use bitvec::prelude::*;

use crate::alignment::align_up;

const PAGE_SIZE: usize = 4096;

pub struct BitMapAllocator {
    bitmap: &'static mut BitSlice<u8, LocalBits>,
    /// Raw pointer to bitmap bytes for fast single-page operations.
    raw_bitmap: *mut u8,
    raw_bitmap_len: usize,
    base: usize,
    end: usize,
    next_hint: usize,
}

// Safety: BitMapAllocator is only accessed under a SpinLock in page_allocator.rs.
unsafe impl Send for BitMapAllocator {}

impl BitMapAllocator {
    /// # Safety
    ///
    /// The caller must ensure that the memory passed to this function is
    /// aligned to a page boundary.
    pub unsafe fn new(base: *mut u8, base_paddr: usize, len: usize) -> BitMapAllocator {
        let num_pages = align_up(len, PAGE_SIZE) / PAGE_SIZE;
        let bitmap_reserved_len = align_up(num_pages / 8, PAGE_SIZE);
        let bitmap_actual_len = (num_pages / 8) - (bitmap_reserved_len / PAGE_SIZE);
        let bitmap =
            BitSlice::from_slice_mut(unsafe { core::slice::from_raw_parts_mut(base, bitmap_actual_len) });

        debug_assert!(bitmap_reserved_len >= bitmap_actual_len);
        bitmap.fill(false);

        let raw_bitmap = base;
        let raw_bitmap_len = bitmap_actual_len;

        BitMapAllocator {
            bitmap,
            raw_bitmap,
            raw_bitmap_len,
            base: base_paddr + bitmap_reserved_len,
            end: base_paddr + len - bitmap_reserved_len,
            next_hint: 0,
        }
    }

    pub fn num_total_pages(&self) -> usize {
        (self.end - self.base) / PAGE_SIZE
    }

    pub fn includes(&mut self, ptr: usize) -> bool {
        self.base <= ptr && ptr < self.end
    }

    /// Ultra-fast single-page allocation operating directly on raw bytes.
    /// Bypasses bitvec abstraction overhead for the critical hot path.
    #[inline(always)]
    pub fn alloc_one(&mut self) -> Option<usize> {
        let raw = self.raw_bitmap;
        let raw_len = self.raw_bitmap_len;
        let total_bits = self.bitmap.len();

        // Start scanning from the byte containing next_hint.
        let mut byte_idx = self.next_hint / 8;
        let bit_off = self.next_hint % 8;

        // Check the first byte, masking out bits before our hint position.
        if byte_idx < raw_len {
            let byte = unsafe { *raw.add(byte_idx) } | ((1u8 << bit_off) - 1);
            if byte != 0xff {
                let bit = (!byte).trailing_zeros() as usize;
                let page_idx = byte_idx * 8 + bit;
                if page_idx < total_bits {
                    unsafe { *raw.add(byte_idx) |= 1u8 << bit; }
                    self.next_hint = page_idx + 1;
                    return Some(self.base + page_idx * PAGE_SIZE);
                }
            }
            byte_idx += 1;
        }

        // Scan forward through subsequent bytes.
        while byte_idx < raw_len {
            let byte = unsafe { *raw.add(byte_idx) };
            if byte != 0xff {
                let bit = (!byte).trailing_zeros() as usize;
                let page_idx = byte_idx * 8 + bit;
                if page_idx < total_bits {
                    unsafe { *raw.add(byte_idx) |= 1u8 << bit; }
                    self.next_hint = page_idx + 1;
                    return Some(self.base + page_idx * PAGE_SIZE);
                }
                return None;
            }
            byte_idx += 1;
        }

        // Wrap around: scan from beginning up to original start.
        let wrap_end = self.next_hint / 8;
        byte_idx = 0;
        while byte_idx <= wrap_end && byte_idx < raw_len {
            let byte = unsafe { *raw.add(byte_idx) };
            if byte != 0xff {
                let bit = (!byte).trailing_zeros() as usize;
                let page_idx = byte_idx * 8 + bit;
                if page_idx < total_bits {
                    unsafe { *raw.add(byte_idx) |= 1u8 << bit; }
                    self.next_hint = page_idx + 1;
                    return Some(self.base + page_idx * PAGE_SIZE);
                }
            }
            byte_idx += 1;
        }

        None
    }

    /// Fast single-page free operating directly on raw bytes.
    #[inline(always)]
    pub fn free_one(&mut self, ptr: usize) {
        let page_idx = (ptr - self.base) / PAGE_SIZE;
        let byte_idx = page_idx / 8;
        let bit = page_idx % 8;
        unsafe {
            let byte_ptr = self.raw_bitmap.add(byte_idx);
            debug_assert!(*byte_ptr & (1u8 << bit) != 0, "double free");
            *byte_ptr &= !(1u8 << bit);
        }
    }

    pub fn alloc_pages(&mut self, order: usize) -> Option<usize> {
        // Fast path for single-page allocation.
        if order == 0 {
            return self.alloc_one();
        }

        let num_pages = 1 << order;
        let bitmap = &mut *self.bitmap;

        // Next-fit allocation: start from hint position for better cache behavior
        let mut off = self.next_hint;
        let mut wrapped = false;

        loop {
            if let Some(first_zero) = bitmap[off..].first_zero() {
                let start = off + first_zero;
                let end = start + num_pages;

                if end > bitmap.len() {
                    // Hit end - wrap around to beginning if not tried yet
                    if wrapped {
                        return None;
                    }
                    off = 0;
                    wrapped = true;
                    continue;
                }

                if bitmap[start..end].not_any() {
                    bitmap[start..end].fill(true);
                    self.next_hint = end;  // Update hint to just after allocation
                    return Some(self.base + start * PAGE_SIZE);
                }

                off = start + 1;
            } else {
                // No zeros from current position - wrap if not tried yet
                if wrapped {
                    return None;
                }
                off = 0;
                wrapped = true;
            }
        }
    }

    pub fn free_pages(&mut self, ptr: usize, order: usize) {
        // Fast path for single-page free.
        if order == 0 {
            self.free_one(ptr);
            return;
        }

        let num_pages = 1 << order;
        let off = (ptr - self.base) / PAGE_SIZE;

        let bitmap = &mut *self.bitmap;

        debug_assert!(bitmap[off..(off + num_pages)].all(), "double free");
        bitmap[off..(off + num_pages)].fill(false);
    }
}
