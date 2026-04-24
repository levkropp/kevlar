// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Re-export initramfs from the kevlar_initramfs service crate.
//!
//! Only `init()` stays here because it uses `include_bytes!` with the
//! build-time `INITRAMFS_PATH` environment variable.
pub use kevlar_initramfs::*;

use alloc::sync::Arc;
use kevlar_platform::{
    address::PAddr,
    arch::PAGE_SIZE,
    page_allocator::{alloc_pages, AllocPageFlags},
    page_refcount::page_ref_init_kernel_image,
};

/// Copy a file's bytes into a freshly-allocated, page-aligned kernel
/// buffer.  Enables the demand-pager's `DIRECT_MAP_ENABLED` fast path
/// for that file by giving it a page-aligned `data_vaddr()`.  The
/// allocated pages are marked with `PAGE_REF_KERNEL_IMAGE` so fork
/// CoW walks and drop paths short-circuit them.
///
/// The returned slice is `&'static [u8]` because the underlying
/// kernel pages are never freed (they back the initramfs for the
/// lifetime of the kernel).
fn align_file_data(data: &'static [u8]) -> &'static [u8] {
    if data.is_empty() {
        return data;
    }
    let n_pages = (data.len() + PAGE_SIZE - 1) / PAGE_SIZE;
    // buddy_alloc tops out at MAX_ORDER = 10 (1024 pages = 4 MB) per
    // contiguous allocation.  Files bigger than that (e.g. apk.static
    // at 4.4 MB) stay where they are — DIRECT_MAP simply won't apply
    // for them; demand-paging still works via the copy path.
    if n_pages > 1024 {
        return data;
    }
    let paddr: PAddr = match alloc_pages(n_pages, AllocPageFlags::KERNEL) {
        Ok(p) => p,
        Err(_) => {
            // Fallback: leave data where it is (DIRECT_MAP won't apply).
            return data;
        }
    };
    // Mark every page with the sentinel so future page_ref_inc /
    // page_ref_dec short-circuit.
    for i in 0..n_pages {
        page_ref_init_kernel_image(paddr.add(i * PAGE_SIZE));
    }
    #[allow(unsafe_code)]
    unsafe {
        let dst = paddr.as_mut_ptr::<u8>();
        core::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
        core::slice::from_raw_parts(dst as *const u8, data.len())
    }
}

pub fn init() {
    INITRAM_FS.init(|| {
        let image = include_bytes!(concat!("../../", env!("INITRAMFS_PATH")));
        if image.is_empty() {
            panic!("initramfs is not embedded");
        }

        Arc::new(InitramFs::new_with_align(image, align_file_data))
    });
}
