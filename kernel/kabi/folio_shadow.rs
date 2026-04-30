// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! VMEMMAP shadow region for Linux folio compatibility (Phase 3b).
//!
//! ## Why this exists
//!
//! Linux's inline `kmap_local_page(folio)` on arm64 expands to:
//!
//! ```text
//!     idx = (folio - VMEMMAP_START) / sizeof(struct page)
//!     va  = PAGE_OFFSET + idx * PAGE_SIZE
//! ```
//!
//! Verified by disassembling `erofs_bread+0x138` in
//! `linux-modules-7.0.0-14-generic`'s `erofs.ko`:
//!
//! ```asm
//! mov   x1, #0x40000000           ; movz lsl #16
//! movk  x1, #0x200, lsl #32       ; x1 = 0x0200_4000_0000 = -VMEMMAP_START
//! add   x2, x0, x1                ; x2 = page - VMEMMAP_START
//! mov   x0, #0xfffffff000000000   ; preshifted PAGE_OFFSET
//! add   x2, x0, x2, lsr #6        ; x2 = preshift_PAGE_OFFSET + idx
//! lsl   x2, x2, #12               ; result = PAGE_OFFSET + idx * 4096
//! ```
//!
//! Decoded constants:
//!   * `VMEMMAP_START`     = `0xffff_fdff_c000_0000`
//!   * `PAGE_OFFSET`       = `0xffff_0000_0000_0000`  ← **identical to
//!     Kevlar's `KERNEL_BASE_ADDR`** (after preshift+lsl wraparound)
//!   * `sizeof(struct page)` = 64
//!
//! ## The play
//!
//! Linux's runtime PAGE_OFFSET equals Kevlar's KERNEL_BASE_ADDR.  So
//! if we hand erofs a "fake page" pointer at
//! `VMEMMAP_START + paddr/64`, Linux's inline math computes exactly
//! `KERNEL_BASE + paddr` — the Kevlar direct-map VA where the data
//! buffer lives.  No special data-buffer placement needed: any
//! `kmalloc`'d data buffer at Kevlar paddr P is naturally accessible
//! at `KERNEL_BASE + P`.
//!
//! ## What this module does
//!
//! 1. At boot: allocate one 4 KiB physical page as the "shadow page"
//!    and install a TTBR1 mapping at VA `VMEMMAP_START` pointing at
//!    it.  The shadow page holds the folio (= struct page) header
//!    fields erofs reads — `flags`, `mapping`, `index` — at offsets
//!    matching Linux's struct page layout.
//!
//! 2. Per-folio synthesis: for each `read_cache_folio` call, we
//!    allocate a 4 KiB data buffer at paddr P, fill it with file
//!    data, populate the corresponding 64-byte slot in the shadow
//!    page (`VMEMMAP_START + P/64`), and return that fake_page_va.
//!
//! 3. One 4 KiB shadow page covers `4096 / 64 = 64` fake_page slots,
//!    each describing a 4 KiB data page → 256 KiB of paddr coverage.
//!    That's enough for a small read-only mount.  Extending requires
//!    additional shadow PT pages but no design changes.
//!
//! ## Coverage limits (v1)
//!
//! Data buffers must come from a paddr range whose `paddr/64` falls
//! inside the mapped shadow region.  v1 reserves a contiguous
//! 256 KiB physical range at boot for this purpose; folio
//! allocations come from there.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use kevlar_platform::address::PAddr;
#[cfg(target_arch = "aarch64")]
use kevlar_platform::page_allocator::{alloc_pages, AllocPageFlags};

/// VA where Linux's compiled `kmap_local_page` math expects the
/// VMEMMAP region (`struct page` array) to start.  Decoded from
/// erofs.ko disasm; matches arm64 7.0's `VMEMMAP_END - VMEMMAP_SIZE`
/// for the LPA2-fallback path on 48-bit-VA hardware.
pub const VMEMMAP_START: u64 = 0xffff_fdff_c000_0000;

/// Linux's PAGE_OFFSET as it appears at runtime in compiled code.
/// Identical to Kevlar's `KERNEL_BASE_ADDR` — the math just lines
/// up.  Documented here so the equivalence is explicit.
pub const LINUX_PAGE_OFFSET: u64 = 0xffff_0000_0000_0000;

/// `sizeof(struct page)` on Linux 7.0 arm64.  This is the divisor
/// in the inline `kmap_local_page` arithmetic; verified via the
/// `lsr #6` in erofs.ko.
pub const SIZEOF_STRUCT_PAGE: u64 = 64;

const PAGE_SIZE: usize = 4096;

// ── ARM64 page-table descriptor bits (subset; mirrors paging.rs) ─

const DESC_VALID: u64 = 1 << 0;
const DESC_TABLE: u64 = 1 << 1;
const DESC_PAGE: u64 = 1 << 1; // Page descriptor at L3 (same encoding).
const ATTR_IDX_NORMAL: u64 = 1 << 2; // MAIR index 1 = Normal WB
const ATTR_SH_ISH: u64 = 3 << 8;
const ATTR_AF: u64 = 1 << 10;
const ATTR_PXN: u64 = 1 << 53;
const ATTR_UXN: u64 = 1 << 54;

/// Physical address of the shadow page (the leaf data page that
/// holds fake_page entries).  Set at init time, read on every
/// `read_cache_folio`.
static SHADOW_PADDR: AtomicU64 = AtomicU64::new(0);

/// Bump pointer through the reserved data-buffer region.  Each
/// allocation hands back the next 4 KiB.  v1 single-mount, no free.
static DATA_BUMP_PADDR: AtomicU64 = AtomicU64::new(0);
static DATA_BUMP_END: AtomicU64 = AtomicU64::new(0);

/// Counter for diagnostics — number of fake_page slots populated.
static FAKE_PAGES_USED: AtomicUsize = AtomicUsize::new(0);

/// Shadow region byte size (covers 64 fake_page entries).
const SHADOW_BYTES: usize = PAGE_SIZE;

/// Reserved data-buffer region — one shadow page describes
/// `(SHADOW_BYTES / SIZEOF_STRUCT_PAGE) * PAGE_SIZE` bytes of paddr
/// coverage = 256 KiB.
const DATA_REGION_BYTES: usize =
    (SHADOW_BYTES / SIZEOF_STRUCT_PAGE as usize) * PAGE_SIZE;

#[cfg(target_arch = "aarch64")]
unsafe fn write_kernel_pte(va_ptr: *mut u64, val: u64) {
    unsafe {
        core::ptr::write_volatile(va_ptr, val);
        core::arch::asm!("dsb ishst", options(nostack));
    }
}

/// Walk the kernel PGD (TTBR1) to install a leaf mapping for `vaddr`
/// pointing at `paddr` with normal WB-cacheable attributes.  Allocates
/// PUD/PMD/PT pages as needed.
#[cfg(target_arch = "aarch64")]
fn install_kernel_4k_mapping(vaddr: u64, paddr: u64) {
    let attrs_table: u64 = DESC_VALID | DESC_TABLE;
    let attrs_leaf: u64 = DESC_VALID
        | DESC_PAGE
        | ATTR_IDX_NORMAL
        | ATTR_SH_ISH
        | ATTR_AF
        | ATTR_PXN
        | ATTR_UXN;

    // Read TTBR1_EL1 — kernel PGD physical address (low 48 bits;
    // top 16 bits are CnP / reserved).
    let ttbr1: u64;
    unsafe {
        core::arch::asm!(
            "mrs {0}, ttbr1_el1",
            out(reg) ttbr1,
            options(nostack, nomem),
        );
    }
    let pgd_paddr = ttbr1 & 0x0000_FFFF_FFFF_F000;
    let pgd_va = PAddr::new(pgd_paddr as usize).as_vaddr().value() as *mut u64;

    let pgd_idx = ((vaddr >> 39) & 0x1ff) as usize;
    let pud_idx = ((vaddr >> 30) & 0x1ff) as usize;
    let pmd_idx = ((vaddr >> 21) & 0x1ff) as usize;
    let pt_idx = ((vaddr >> 12) & 0x1ff) as usize;

    // ── PGD[idx] → PUD ───────────────────────────────────────────
    let pgd_entry_ptr = unsafe { pgd_va.add(pgd_idx) };
    let pgd_entry = unsafe { core::ptr::read_volatile(pgd_entry_ptr) };
    let pud_paddr = if pgd_entry & DESC_VALID == 0 {
        let pud = alloc_pages(1, AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK)
            .expect("folio_shadow: alloc PUD");
        unsafe { pud.as_mut_ptr::<u8>().write_bytes(0, PAGE_SIZE); }
        let p = pud.value() as u64;
        unsafe { write_kernel_pte(pgd_entry_ptr, p | attrs_table); }
        p
    } else {
        pgd_entry & 0x0000_FFFF_FFFF_F000
    };
    let pud_va = PAddr::new(pud_paddr as usize).as_vaddr().value() as *mut u64;

    // ── PUD[idx] → PMD ───────────────────────────────────────────
    let pud_entry_ptr = unsafe { pud_va.add(pud_idx) };
    let pud_entry = unsafe { core::ptr::read_volatile(pud_entry_ptr) };
    let pmd_paddr = if pud_entry & DESC_VALID == 0 {
        let pmd = alloc_pages(1, AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK)
            .expect("folio_shadow: alloc PMD");
        unsafe { pmd.as_mut_ptr::<u8>().write_bytes(0, PAGE_SIZE); }
        let p = pmd.value() as u64;
        unsafe { write_kernel_pte(pud_entry_ptr, p | attrs_table); }
        p
    } else {
        pud_entry & 0x0000_FFFF_FFFF_F000
    };
    let pmd_va = PAddr::new(pmd_paddr as usize).as_vaddr().value() as *mut u64;

    // ── PMD[idx] → PT ────────────────────────────────────────────
    let pmd_entry_ptr = unsafe { pmd_va.add(pmd_idx) };
    let pmd_entry = unsafe { core::ptr::read_volatile(pmd_entry_ptr) };
    let pt_paddr = if pmd_entry & DESC_VALID == 0 {
        let pt = alloc_pages(1, AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK)
            .expect("folio_shadow: alloc PT");
        unsafe { pt.as_mut_ptr::<u8>().write_bytes(0, PAGE_SIZE); }
        let p = pt.value() as u64;
        unsafe { write_kernel_pte(pmd_entry_ptr, p | attrs_table); }
        p
    } else {
        pmd_entry & 0x0000_FFFF_FFFF_F000
    };
    let pt_va = PAddr::new(pt_paddr as usize).as_vaddr().value() as *mut u64;

    // ── PT[idx] → 4 KiB leaf ─────────────────────────────────────
    let pt_entry_ptr = unsafe { pt_va.add(pt_idx) };
    unsafe { write_kernel_pte(pt_entry_ptr, paddr | attrs_leaf); }

    // Final fence + TLB invalidate by VA at EL1, broadcast.
    unsafe {
        core::arch::asm!(
            "dsb ishst",
            "tlbi vaae1is, {x}",
            "dsb ish",
            "isb",
            x = in(reg) (vaddr >> 12) & 0x0000_FFFF_FFFF_FFFF,
            options(nostack),
        );
    }
}

#[cfg(not(target_arch = "aarch64"))]
fn install_kernel_4k_mapping(_vaddr: u64, _paddr: u64) {
    // No-op on non-arm64; folio_shadow is arm64-specific.
}

/// Initialise the VMEMMAP shadow region.
///
/// Steps:
///   1. Reserve a contiguous 256 KiB physical data-buffer region
///      (64 4 KiB pages — what one shadow page can describe).
///   2. Compute the corresponding fake_page VA range
///      (`VMEMMAP_START + data_base/64` through `+ data_end/64`).
///      This is exactly 4 KiB wide; if `data_base/64` is page-aligned
///      we need 1 shadow page, otherwise 2 (range crosses a boundary).
///   3. Allocate shadow page(s) and map them at the computed VA(s)
///      in TTBR1 with normal WB attrs.  The shadow page will hold
///      the fake_page header entries written by `alloc_folio`.
///
/// Call from `kabi::init()` after the page allocator is up.
#[cfg(target_arch = "aarch64")]
pub fn init() {
    // 1. Reserve the data-buffer pool.
    let data = match alloc_pages(
        DATA_REGION_BYTES / PAGE_SIZE,
        AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
    ) {
        Ok(p) => p,
        Err(_) => {
            log::warn!("kabi: folio_shadow init: alloc data region failed");
            return;
        }
    };
    let data_base = data.value() as u64;
    let data_end = data_base + DATA_REGION_BYTES as u64;
    DATA_BUMP_PADDR.store(data_base, Ordering::Release);
    DATA_BUMP_END.store(data_end, Ordering::Release);

    // 2. Compute fake_page VA range and required shadow page span.
    let fake_base = VMEMMAP_START + data_base / SIZEOF_STRUCT_PAGE;
    let fake_end = VMEMMAP_START + data_end / SIZEOF_STRUCT_PAGE;
    let map_start = fake_base & !(PAGE_SIZE as u64 - 1);
    let map_end = (fake_end + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);
    let num_shadow_pages = ((map_end - map_start) / PAGE_SIZE as u64) as usize;

    // 3. Allocate + map the shadow page(s).  Records the first
    //    shadow paddr for diagnostics; shadow pages are zero-filled.
    let mut first_shadow: u64 = 0;
    for i in 0..num_shadow_pages {
        let shadow = match alloc_pages(1, AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK) {
            Ok(p) => p,
            Err(_) => {
                log::warn!("kabi: folio_shadow init: alloc shadow page {} failed", i);
                return;
            }
        };
        unsafe { shadow.as_mut_ptr::<u8>().write_bytes(0, PAGE_SIZE); }
        if i == 0 {
            first_shadow = shadow.value() as u64;
            SHADOW_PADDR.store(first_shadow, Ordering::Release);
        }
        let va = map_start + (i as u64) * PAGE_SIZE as u64;
        install_kernel_4k_mapping(va, shadow.value() as u64);
    }

    log::info!(
        "kabi: folio_shadow init: data_paddr=[{:#x}..{:#x}) \
         fake_page=[{:#x}..{:#x}) mapped {} shadow page(s) at \
         VMEMMAP+{:#x} (first_shadow_paddr={:#x})",
        data_base, data_end,
        fake_base, fake_end,
        num_shadow_pages,
        map_start - VMEMMAP_START,
        first_shadow,
    );
}

#[cfg(not(target_arch = "aarch64"))]
pub fn init() {}

/// Allocate a folio backed by:
///   * A 4 KiB data buffer at paddr P (in our reserved region)
///   * A fake_page entry at `VMEMMAP_START + P/64` populated with
///     folio header fields (`flags`, `mapping`, `index`).
///
/// Returns `(fake_page_va, data_va)` — the caller fills `data_va`
/// with file data and returns `fake_page_va` to erofs.  Inline
/// `kmap_local_page(fake_page_va)` will compute exactly `data_va`.
///
/// Returns `None` if the data-buffer pool is exhausted.
pub fn alloc_folio(
    flags: u64,
    mapping: *mut c_void,
    index: u64,
) -> Option<(u64, *mut u8)> {
    let bump = DATA_BUMP_PADDR.fetch_add(PAGE_SIZE as u64, Ordering::AcqRel);
    let end = DATA_BUMP_END.load(Ordering::Acquire);
    if bump + PAGE_SIZE as u64 > end {
        // Bump pointer over-shot; back it off so subsequent calls
        // also see the failure cleanly.  Single-mount v1 — we
        // tolerate the leak of the AcqRel slot.
        DATA_BUMP_PADDR.store(end, Ordering::Release);
        log::warn!(
            "kabi: folio_shadow: data pool exhausted ({} entries used)",
            FAKE_PAGES_USED.load(Ordering::Relaxed),
        );
        return None;
    }
    let data_paddr = bump;
    let data_va = PAddr::new(data_paddr as usize).as_vaddr().value() as *mut u8;
    unsafe { core::ptr::write_bytes(data_va, 0, PAGE_SIZE); }

    // Compute fake_page slot inside the shadow page.
    let fake_page_va = VMEMMAP_START + data_paddr / SIZEOF_STRUCT_PAGE;

    // Populate the fake_page header.  Linux 7.0 struct folio layout
    // (include/linux/mm_types.h:401):
    //   +0   memdesc_flags_t flags             (8 bytes)
    //   +8   union { list_head lru, pgmap }    (16 bytes — list_head is
    //                                            { next, prev } = 16 B)
    //   +24  struct address_space *mapping     (8 bytes)
    //   +32  pgoff_t index                     (8 bytes)
    //
    // The fake_page_va lives inside the shadow page that we mapped
    // at VMEMMAP_START in `init()`.  Writes through it land in
    // the shadow page's physical backing.
    //
    // Phase 13 fix: Linux 7.0 has mapping at +24 (not +16), since the
    // lru/pgmap union absorbs 16 bytes after flags.  The pre-Phase-13
    // assumption of +16 worked for erofs's mount path because erofs
    // reads inode->i_mapping directly rather than folio->mapping.
    // ext4_read_folio reads folio->mapping->host as its first action,
    // so the offset has to match Linux's actual layout.
    unsafe {
        let p = fake_page_va as *mut u8;
        core::ptr::write_volatile(p as *mut u64, flags);
        core::ptr::write_volatile(p.add(24) as *mut *mut c_void, mapping);
        core::ptr::write_volatile(p.add(32) as *mut u64, index);
    }

    FAKE_PAGES_USED.fetch_add(1, Ordering::Relaxed);
    Some((fake_page_va, data_va))
}

/// PG_uptodate flag — bit 3 of `folio->flags`.  Tells callers the
/// folio's data is valid; they skip calling `read_folio` and use
/// the data directly.
pub const PG_UPTODATE: u64 = 1 << 3;

/// Inverse of the inline `kmap_local_page` math erofs and ext4 use:
///   data_va = LINUX_PAGE_OFFSET + ((fake_page_va - VMEMMAP_START)
///             / sizeof(struct page)) * PAGE_SIZE
///
/// Used by `filemap_read` to compute the kernel VA holding the
/// actual page contents from a folio (fake-page) pointer.
pub fn folio_to_data_va(fake_page_va: u64) -> *mut u8 {
    let idx = (fake_page_va.wrapping_sub(VMEMMAP_START)) / SIZEOF_STRUCT_PAGE;
    let data_va = LINUX_PAGE_OFFSET + idx * PAGE_SIZE as u64;
    data_va as *mut u8
}
