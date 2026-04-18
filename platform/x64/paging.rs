// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use super::{PAGE_SIZE, KERNEL_STRAIGHT_MAP_PADDR_END};
use alloc::vec::Vec;
use crate::address::{PAddr, UserVAddr};
use crate::page_allocator::{alloc_pages, AllocPageFlags, PageAllocError};
use crate::spinlock::SpinLock;
use bitflags::bitflags;
use core::{
    debug_assert,
    ptr::{self, NonNull},
};
use kevlar_utils::alignment::is_aligned;

// ── Page table page pool ──────────────────────────────────────────────
// Pre-allocated pool of zeroed 4KB pages for page table allocation.
// Eliminates buddy allocator overhead (~70ns/alloc) during fork.
const PT_POOL_MAX: usize = 32;
static PT_PAGE_POOL: SpinLock<Vec<PAddr>> = SpinLock::new(Vec::new());

/// Magic cookie stored at entry 511 of every PT page (offset 0xFF8).
/// User page tables never use entry 511 (it maps vaddr 0x7FFFFFFFE000+
/// at level 1, which is inside the kernel half). If a PT page's cookie
/// is overwritten, another subsystem is writing to this page.
const PT_PAGE_MAGIC: u64 = 0xBEEF_CA11_DEAD_F00D;

fn alloc_pt_page() -> Result<PAddr, PageAllocError> {
    if let Some(page) = PT_PAGE_POOL.lock_no_irq().pop() {
        // Verify cookie is intact
        let ptr = page.as_mut_ptr::<u64>();
        let cookie = unsafe { *ptr.offset(511) };
        if cookie != PT_PAGE_MAGIC {
            panic!("PT page {:#x} cookie corrupted: {:#x} (expected {:#x})",
                page.value(), cookie, PT_PAGE_MAGIC);
        }
        return Ok(page);
    }
    let page = alloc_pages(1, AllocPageFlags::KERNEL)?;
    // Set cookie on fresh page
    unsafe { *page.as_mut_ptr::<u64>().offset(511) = PT_PAGE_MAGIC; }
    Ok(page)
}

/// Flush stale TLB entries on every CPU before tearing down a page table.
/// Without this, freed PT pages and freed user data pages can be re-issued
/// (to PT pool, to slab, to user mmap) while another CPU still holds TLB
/// entries pointing at them. A subsequent write through the stale entry
/// corrupts the new owner.
///
/// We do two things:
/// 1. Bump the global PCID generation so any future `PageTable::switch()`
///    forces a full CR3 reload (covers entries cached for the dropped PCID
///    that survive in CPUs not currently scheduling).
/// 2. Send a TLB IPI to all other CPUs telling them to invalidate ALL
///    PCIDs (not just the current one) immediately. The IPI is skipped if
///    the caller has interrupts disabled (would deadlock waiting for ACK).
///    Local CPU also flushes via INVPCID type=3.
fn flush_tlb_for_teardown() {
    bump_global_pcid_generation();
    if super::interrupts_enabled() {
        super::apic::tlb_remote_flush_all_pcids();
        flush_all_pcids();
    }
}

/// Invalidate all TLB entries on the current CPU for all PCIDs (excluding
/// global mappings). Uses INVPCID type=3 if supported; otherwise toggles
/// CR4.PCIDE which has the same effect.
pub fn flush_all_pcids() {
    if super::boot::INVPCID_SUPPORTED.load(core::sync::atomic::Ordering::Relaxed) {
        // INVPCID type=3: invalidate all contexts EXCEPT global mappings.
        // The descriptor at [rdi] is ignored for type 3 but must be a valid
        // 16-byte aligned address.
        let descriptor: [u64; 2] = [0, 0];
        unsafe {
            core::arch::asm!(
                "invpcid {0}, [{1}]",
                in(reg) 3u64,
                in(reg) &descriptor,
                options(nostack, preserves_flags),
            );
        }
    } else {
        // Fallback for hardware without INVPCID: toggle CR4.PCIDE off and on.
        // Clearing PCIDE flushes all TLB entries (including non-global).
        // We then re-set PCIDE and re-load CR3.
        unsafe {
            let cr4 = x86::controlregs::cr4();
            let cr3 = x86::controlregs::cr3();
            // Clearing PCIDE requires CR3 to have PCID bits cleared.
            x86::controlregs::cr3_write(cr3 & !0xFFF);
            x86::controlregs::cr4_write(cr4 & !x86::controlregs::Cr4::CR4_ENABLE_PCID);
            x86::controlregs::cr4_write(cr4);
            x86::controlregs::cr3_write(cr3);
        }
    }
}

/// Return a page table page to the pool (or free it if pool is full).
#[inline]
fn free_pt_page(paddr: PAddr) {
    // Write cookie before returning to pool
    unsafe { *paddr.as_mut_ptr::<u64>().offset(511) = PT_PAGE_MAGIC; }
    let mut pool = PT_PAGE_POOL.lock_no_irq();
    if pool.len() < PT_POOL_MAX {
        pool.push(paddr);
    } else {
        drop(pool);
        crate::page_allocator::free_pages(paddr, 1);
    }
}

const ENTRIES_PER_TABLE: isize = 512;
type PageTableEntry = u64;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct PageAttrs: u64 {
        const PRESENT = 1 << 0;
        const WRITABLE = 1 << 1;
        const USER = 1 << 2;
        const WRITE_THROUGH = 1 << 3; // PWT — write-through caching
        const CACHE_DISABLE = 1 << 4; // PCD — page cache disable (uncacheable)
        const HUGE_PAGE = 1 << 7; // PS bit — marks a 2MB page in a PDE
        const NO_EXECUTE = 1 << 63;
    }
}

/// 2MB huge page size (512 × 4KB pages).
pub const HUGE_PAGE_SIZE: usize = 512 * PAGE_SIZE;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct PageFaultReason: u32 {
        const PRESENT = 1 << 0;
        const CAUSED_BY_WRITE = 1 << 1;
        const CAUSED_BY_USER = 1 << 2;
        const RESERVED_WRITE = 1 << 3;
        const CAUSED_BY_INST_FETCH = 1 << 4;
    }
}

#[inline(always)]
fn entry_paddr(entry: PageTableEntry) -> PAddr {
    // Mask: bits 12-51 only (40-bit physical address).
    // Bits 52-58 are available for OS, bits 59-62 are reserved/PKEY,
    // bit 63 is NX. Including them would produce invalid physical addresses.
    PAddr::new((entry & 0x000f_ffff_ffff_f000) as usize)
}

#[inline(always)]
fn entry_flags(entry: PageTableEntry) -> PageTableEntry {
    // Extract everything that's NOT the physical address: bits 0-11, 52-63.
    entry & !0x000f_ffff_ffff_f000
}

#[inline(always)]
fn nth_level_table_index(vaddr: UserVAddr, level: usize) -> isize {
    ((vaddr.value() >> ((((level) - 1) * 9) + 12)) & 0x1ff) as isize
}

/// Walk the page table hierarchy to find the PTE for `vaddr`.
/// If `allocate` is true, allocate missing intermediate tables.
///
/// Hot path: called 17 times per page fault (1 primary + 16 fault-around).
/// Optimized: inline, no conditional write-back for existing entries.
#[inline(always)]
fn traverse(
    pml4: PAddr,
    vaddr: UserVAddr,
    allocate: bool,
    _attrs: PageAttrs,
) -> Option<NonNull<PageTableEntry>> {
    debug_assert!(is_aligned(vaddr.value(), PAGE_SIZE));
    // Intermediate page table entries (PML4E, PDPTE, PDE) must use permissive
    // flags so that restrictive leaf PTE attrs (e.g. read-only, NX) don't
    // propagate upward and block access to sibling entries in the same table.
    // On x86_64, effective permissions are the intersection of all levels, so
    // intermediates need WRITABLE and no NO_EXECUTE to avoid over-restricting.
    let intermediate_attrs = PageAttrs::PRESENT | PageAttrs::USER | PageAttrs::WRITABLE;
    let mut table = pml4.as_mut_ptr::<PageTableEntry>();
    for level in (2..=4).rev() {
        let index = nth_level_table_index(vaddr, level);
        let entry = unsafe { table.add(index as usize) };
        let entry_val = unsafe { *entry };
        let table_paddr = entry_paddr(entry_val);
        if table_paddr.value() == 0 {
            if !allocate {
                return None;
            }

            let new_table =
                alloc_pages(1, AllocPageFlags::KERNEL).expect("failed to allocate page table");
            unsafe {
                *entry = new_table.value() as u64 | intermediate_attrs.bits()
            };
            table = new_table.as_mut_ptr::<PageTableEntry>();
        } else {
            // At level 2 (PD), if the PDE has the PS (huge page) bit set,
            // this is a 2MB leaf — there is no level-1 page table to descend into.
            if level == 2 && (entry_val & PageAttrs::HUGE_PAGE.bits()) != 0 {
                return None;
            }
            // Ensure intermediate entries have permissive flags.  A previous
            // mapping with restrictive attrs might have cleared WRITABLE or
            // set NO_EXECUTE, which would over-restrict all sibling PTEs.
            let expected = table_paddr.value() as u64 | intermediate_attrs.bits();
            if entry_val != expected {
                unsafe { *entry = expected; }
            }
            table = table_paddr.as_mut_ptr::<PageTableEntry>();
        }
    }

    unsafe {
        Some(NonNull::new_unchecked(
            table.add(nth_level_table_index(vaddr, 1) as usize),
        ))
    }
}

/// Walk PML4→PDPT→PD to find the leaf Page Table base address.
/// Returns a raw pointer to the start of the 512-entry PT.
/// Unlike `traverse`, does NOT index into the final PT level.
#[inline(always)]
fn traverse_to_pt(
    pml4: PAddr,
    vaddr: UserVAddr,
    allocate: bool,
    _attrs: PageAttrs,
) -> Option<*mut PageTableEntry> {
    let intermediate_attrs = PageAttrs::PRESENT | PageAttrs::USER | PageAttrs::WRITABLE;
    let mut table = pml4.as_mut_ptr::<PageTableEntry>();
    for level in (2..=4).rev() {
        let index = nth_level_table_index(vaddr, level);
        let entry = unsafe { table.add(index as usize) };
        let entry_val = unsafe { *entry };
        let table_paddr = entry_paddr(entry_val);
        if table_paddr.value() == 0 {
            if !allocate {
                return None;
            }
            let new_table =
                alloc_pages(1, AllocPageFlags::KERNEL).expect("failed to allocate page table");
            unsafe { *entry = new_table.value() as u64 | intermediate_attrs.bits() };
            table = new_table.as_mut_ptr::<PageTableEntry>();
        } else {
            let expected = table_paddr.value() as u64 | intermediate_attrs.bits();
            if entry_val != expected {
                unsafe { *entry = expected; }
            }
            table = table_paddr.as_mut_ptr::<PageTableEntry>();
        }
    }
    Some(table)
}

/// Compute the leaf (level-1) page table index for a virtual address.
#[inline(always)]
fn leaf_pt_index(vaddr_value: usize) -> usize {
    (vaddr_value >> 12) & 0x1FF
}

/// Walk PML4→PDPT→PD to find the PDE (Page Directory Entry) for `vaddr`.
/// Returns a raw pointer to the PDE. Used for 2MB huge page operations.
#[inline(always)]
fn traverse_to_pd(
    pml4: PAddr,
    vaddr: UserVAddr,
    allocate: bool,
    _attrs: PageAttrs,
) -> Option<*mut PageTableEntry> {
    let intermediate_attrs = PageAttrs::PRESENT | PageAttrs::USER | PageAttrs::WRITABLE;
    let mut table = pml4.as_mut_ptr::<PageTableEntry>();
    // Walk levels 4 (PML4) and 3 (PDPT) to reach level 2 (PD).
    for level in (3..=4).rev() {
        let index = nth_level_table_index(vaddr, level);
        let entry = unsafe { table.add(index as usize) };
        let entry_val = unsafe { *entry };
        let table_paddr = entry_paddr(entry_val);
        if table_paddr.value() == 0 {
            if !allocate {
                return None;
            }
            let new_table =
                alloc_pages(1, AllocPageFlags::KERNEL).expect("failed to allocate page table");
            unsafe { *entry = new_table.value() as u64 | intermediate_attrs.bits() };
            table = new_table.as_mut_ptr::<PageTableEntry>();
        } else {
            let expected = table_paddr.value() as u64 | intermediate_attrs.bits();
            if entry_val != expected {
                unsafe { *entry = expected; }
            }
            table = table_paddr.as_mut_ptr::<PageTableEntry>();
        }
    }
    // `table` now points to the PD. Return pointer to the specific PDE.
    let index = nth_level_table_index(vaddr, 2);
    Some(unsafe { table.add(index as usize) })
}

/// Check whether a PDE has the PS (huge page) bit set.
#[inline(always)]
fn is_huge_page_pde(entry: PageTableEntry) -> bool {
    (entry & PageAttrs::HUGE_PAGE.bits()) != 0
        && (entry & PageAttrs::PRESENT.bits()) != 0
}

/// Split a 2MB huge page PDE into 512 × 4KB PTEs.
///
/// Allocates a new page table, populates it with 512 PTEs pointing to the
/// constituent 4KB frames (preserving flags minus PS), and replaces the PDE.
/// Returns the physical address of the huge page's base frame.
fn split_huge_page(pml4: PAddr, vaddr: UserVAddr) -> Option<PAddr> {
    let pde_ptr = traverse_to_pd(pml4, vaddr, false,
        PageAttrs::PRESENT | PageAttrs::USER | PageAttrs::WRITABLE)?;
    let pde_val = unsafe { *pde_ptr };
    if !is_huge_page_pde(pde_val) {
        return None;
    }

    let base_paddr = entry_paddr(pde_val);
    // Preserve all flags except PS bit.
    let flags = entry_flags(pde_val) & !PageAttrs::HUGE_PAGE.bits();

    // Allocate a new level-1 page table.
    let pt_paddr = alloc_pages(1, AllocPageFlags::KERNEL)
        .expect("failed to allocate PT for huge page split");
    let pt = pt_paddr.as_mut_ptr::<PageTableEntry>();

    // Fill 512 PTEs, each pointing to base + i*4KB.
    for i in 0..ENTRIES_PER_TABLE as usize {
        let frame_paddr = base_paddr.value() + i * PAGE_SIZE;
        unsafe {
            *pt.add(i) = frame_paddr as u64 | flags;
        }
    }

    // Replace PDE: clear PS, point to new PT. Use intermediate-table flags.
    let pt_flags = PageAttrs::PRESENT | PageAttrs::USER | PageAttrs::WRITABLE;
    unsafe {
        *pde_ptr = pt_paddr.value() as u64 | pt_flags.bits();
    }

    Some(base_paddr)
}

/// Duplicates entires (and referenced memory pages if `level == 1`) in the
/// nth-level page table. Returns the newly created copy of the page table.
///
/// fork(2) uses this funciton to duplicate the memory space.
fn duplicate_table_cow(pml4: PAddr, level: usize) -> Result<PAddr, PageAllocError> {
    duplicate_table(pml4, level)
}

/// Software-defined PTE bit: "was writable before ghost-fork CoW".
/// Used to restore the parent's WRITABLE bits after the ghost child exits/execs.
/// x86_64 bits 9-11 are ignored by hardware and available for OS use.
const PTE_WAS_WRITABLE: u64 = 1 << 10;

/// Like `duplicate_table` but skips all `page_ref_inc` calls.
/// Safe only when the parent is blocked (ghost-fork / vfork semantics).
/// Collects virtual addresses of CoW-marked PTEs into `cow_addrs` for
/// fast targeted restore (avoids O(all_PTEs) scan at exit/exec time).
/// `base_vaddr` tracks the virtual address prefix from ancestor levels.
fn duplicate_table_ghost(
    original_table_paddr: PAddr,
    level: usize,
    base_vaddr: usize,
    cow_addrs: &mut Vec<usize>,
) -> Result<PAddr, PageAllocError> {
    let orig_table = original_table_paddr.as_mut_ptr::<PageTableEntry>();
    let new_table_paddr = alloc_pt_page()?;
    let new_table = new_table_paddr.as_mut_ptr::<PageTableEntry>();

    // Bulk-copy the entire 4KB page table.
    unsafe {
        ptr::copy_nonoverlapping(orig_table, new_table, ENTRIES_PER_TABLE as usize);
    }

    debug_assert!(level > 0);

    if level == 1 {
        // Leaf PTEs: clear WRITABLE (CoW) but DON'T increment refcounts.
        for i in 0..ENTRIES_PER_TABLE {
            let entry = unsafe { *orig_table.offset(i) };
            let paddr = entry_paddr(entry);
            if paddr.is_null() {
                continue;
            }
            let flags = entry_flags(entry);
            if flags & PageAttrs::USER.bits() == 0 {
                continue;
            }
            if flags & PageAttrs::WRITABLE.bits() != 0 {
                let cow_entry = paddr.value() as u64
                    | (flags & !PageAttrs::WRITABLE.bits())
                    | PTE_WAS_WRITABLE;
                unsafe {
                    *orig_table.offset(i) = cow_entry;
                    *new_table.offset(i) = cow_entry;
                }
                cow_addrs.push(base_vaddr | ((i as usize) << 12));
            }
        }
    } else {
        for i in 0..ENTRIES_PER_TABLE {
            if level == 4 && i >= 0x80 {
                continue;
            }
            let entry = unsafe { *orig_table.offset(i) };
            let paddr = entry_paddr(entry);
            if paddr.is_null() {
                continue;
            }
            let shift = (level - 1) * 9 + 12;
            let child_base = base_vaddr | ((i as usize) << shift);
            if level == 2 && is_huge_page_pde(entry) {
                let flags = entry_flags(entry);
                if flags & PageAttrs::USER.bits() == 0 {
                    continue;
                }
                if flags & PageAttrs::WRITABLE.bits() != 0 {
                    let cow_entry = paddr.value() as u64
                        | (flags & !PageAttrs::WRITABLE.bits())
                        | PTE_WAS_WRITABLE;
                    unsafe {
                        *orig_table.offset(i) = cow_entry;
                        *new_table.offset(i) = cow_entry;
                    }
                    // Huge pages: store with bit 0 set as marker (addrs are
                    // always page-aligned so bit 0 is normally 0).
                    cow_addrs.push(child_base | 1);
                }
            } else {
                let new_child_paddr = duplicate_table_ghost(paddr, level - 1, child_base, cow_addrs)?;
                unsafe {
                    *new_table.offset(i) = new_child_paddr.value() as u64 | entry_flags(entry);
                }
            }
        }
    }

    Ok(new_table_paddr)
}

/// Free page table pages from a ghost-forked page table without touching
/// data page refcounts. The data pages are owned by the parent (which is
/// blocked) and were never refcount-incremented during the ghost fork.
fn teardown_table_ghost(table_paddr: PAddr, level: usize) {
    if table_paddr.is_null() {
        return;
    }
    let table = table_paddr.as_mut_ptr::<PageTableEntry>();

    for i in 0..ENTRIES_PER_TABLE {
        let entry = unsafe { *table.offset(i) };
        let paddr = entry_paddr(entry);
        if paddr.is_null() {
            continue;
        }

        if level == 1 {
            // Leaf PTE: the data page belongs to the parent.
            // Pages that were CoW-copied by the ghost child were already
            // remapped to new pages (with refcount 1) and need freeing.
            let flags = entry_flags(entry);
            if flags & PageAttrs::USER.bits() == 0 {
                continue;
            }
            // If this PTE was modified by a CoW fault (WRITABLE set, no
            // PTE_WAS_WRITABLE), it points to a child-owned copy → free it.
            if flags & PageAttrs::WRITABLE.bits() != 0
                && entry & PTE_WAS_WRITABLE == 0
            {
                // This is a CoW-copied page owned by the child.
                let rc = crate::page_refcount::page_ref_count(paddr);
                if rc == 1 {
                    crate::page_refcount::page_ref_dec(paddr);
                    crate::page_allocator::free_pages(paddr, 1);
                }
            }
            // Parent-owned pages (PTE_WAS_WRITABLE set or read-only): don't touch.
        } else if level == 2 && is_huge_page_pde(entry) {
            // Huge pages from ghost fork: parent-owned, don't touch.
        } else {
            if level == 4 && i >= 0x80 {
                continue;
            }
            teardown_table_ghost(paddr, level - 1);
            free_pt_page(paddr);
        }
    }
}

/// Targeted restore: set WRITABLE back on specific PTEs listed in `addrs`.
/// Each address was collected during `duplicate_table_ghost`. Entries with
/// bit 0 set are 2MB huge page addresses (clear bit 0 to get real addr).
/// O(cow_pages) instead of O(all_PTEs) — typically ~200 vs ~10,000.
fn restore_writable_from_list(pml4: PAddr, addrs: &[usize]) {
    for &raw_addr in addrs {
        let is_huge = raw_addr & 1 != 0;
        let vaddr = raw_addr & !1usize;

        let uva = unsafe { UserVAddr::new_unchecked(vaddr) };
        if is_huge {
            // 2MB huge page: traverse to PD base, then index into it.
            if let Some(pde_ptr) = traverse_to_pd(pml4, uva, false, PageAttrs::empty()) {
                let entry = unsafe { *pde_ptr };
                if entry & PTE_WAS_WRITABLE != 0 {
                    unsafe {
                        *pde_ptr = (entry | PageAttrs::WRITABLE.bits()) & !PTE_WAS_WRITABLE;
                    }
                }
            }
        } else {
            // 4KB page: traverse to PTE (level 1) and restore WRITABLE.
            if let Some(mut pte) = unsafe {
                traverse(pml4, uva, false, PageAttrs::empty())
            } {
                let entry = unsafe { *pte.as_ptr() };
                if entry & PTE_WAS_WRITABLE != 0 {
                    unsafe {
                        *pte.as_mut() = (entry | PageAttrs::WRITABLE.bits()) & !PTE_WAS_WRITABLE;
                    }
                }
            }
        }
    }
}

/// Recursively walk a page table, decrementing refcounts on user pages
/// and freeing intermediate page table pages. Called from `teardown_user_pages`.
fn teardown_table(table_paddr: PAddr, level: usize) {
    if table_paddr.value() >= KERNEL_STRAIGHT_MAP_PADDR_END {
        warn!("teardown_table: bad paddr {:#x} at level {}", table_paddr.value(), level);
        return;
    }
    let table = table_paddr.as_mut_ptr::<PageTableEntry>();

    for i in 0..ENTRIES_PER_TABLE {
        let entry = unsafe { *table.offset(i) };
        let paddr = entry_paddr(entry);

        if paddr.is_null() || paddr.value() >= KERNEL_STRAIGHT_MAP_PADDR_END {
            continue;
        }

        if level == 1 {
            // Leaf PTE: decrement refcount, free if last reference.
            let flags = entry_flags(entry);
            if flags & PageAttrs::USER.bits() == 0 {
                continue; // Skip kernel mappings.
            }
            let rc = crate::page_refcount::page_ref_count(paddr);
            if rc == 0 || rc == crate::page_refcount::PAGE_REF_KERNEL_IMAGE {
                continue; // Not tracked or kernel image — skip.
            }
            if crate::page_refcount::page_ref_dec(paddr) {
                crate::page_allocator::free_pages(paddr, 1);
            }
        } else if level == 2 && is_huge_page_pde(entry) {
            // 2MB huge page: decrement refcount on ALL 512 sub-PFNs,
            // free each individually when its refcount reaches 0.
            let flags = entry_flags(entry);
            if flags & PageAttrs::USER.bits() == 0 {
                continue;
            }
            for sub_i in 0..512usize {
                let sub = PAddr::new(paddr.value() + sub_i * PAGE_SIZE);
                let rc = crate::page_refcount::page_ref_count(sub);
                if rc == 0 || rc == crate::page_refcount::PAGE_REF_KERNEL_IMAGE {
                    continue;
                }
                if crate::page_refcount::page_ref_dec(sub) {
                    crate::page_allocator::free_pages(sub, 1);
                }
            }
        } else {
            // Intermediate table: recurse, then free the table page.
            if level == 4 && i >= 0x80 {
                continue; // Skip kernel page table entries.
            }
            teardown_table(paddr, level - 1);
            free_pt_page(paddr);
        }
    }
}

/// Like teardown_table but NEVER frees data pages — only decrements refcounts
/// and frees intermediate page table pages. Safe for forked page tables where
/// data pages may still be referenced by the parent or page cache.
#[inline(never)]
fn teardown_table_dec_only(table_paddr: PAddr, level: usize) {
    // CRITICAL: volatile read prevents the compiler from optimizing
    // away this bounds check.  Without it, the compiler eliminates the
    // comparison and the function GPFs on non-canonical addresses from
    // corrupted PTEs.
    let pv = unsafe { core::ptr::read_volatile(&table_paddr.value()) };
    if pv == 0 || pv >= KERNEL_STRAIGHT_MAP_PADDR_END {
        return;
    }
    let table_ptr_val = pv + super::KERNEL_BASE_ADDR;

    for i in 0..ENTRIES_PER_TABLE {
        // Re-derive table pointer each iteration to prevent register clobber.
        let table = table_ptr_val as *mut PageTableEntry;
        let entry = unsafe { *table.offset(i) };
        let paddr = entry_paddr(entry);

        if paddr.is_null() || paddr.value() >= KERNEL_STRAIGHT_MAP_PADDR_END {
            continue;
        }

        if level == 1 {
            // Leaf PTE: decrement refcount only, never free.
            let flags = entry_flags(entry);
            if flags & PageAttrs::USER.bits() == 0 {
                continue;
            }
            let rc = crate::page_refcount::page_ref_count(paddr);
            if rc == 0 || rc == crate::page_refcount::PAGE_REF_KERNEL_IMAGE {
                continue;
            }
            crate::page_refcount::page_ref_dec(paddr);
        } else if level == 2 && is_huge_page_pde(entry) {
            // 2MB huge page: decrement refcount on sub-PFNs, never free.
            let flags = entry_flags(entry);
            if flags & PageAttrs::USER.bits() == 0 {
                continue;
            }
            for sub_i in 0..512usize {
                let sub = PAddr::new(paddr.value() + sub_i * PAGE_SIZE);
                let rc = crate::page_refcount::page_ref_count(sub);
                if rc == 0 || rc == crate::page_refcount::PAGE_REF_KERNEL_IMAGE {
                    continue;
                }
                crate::page_refcount::page_ref_dec(sub);
            }
        } else {
            // Intermediate table: recurse, then free the table page.
            if level == 4 && i >= 0x80 {
                continue; // Skip kernel page table entries.
            }
            let pv = paddr.value();
            if pv >= KERNEL_STRAIGHT_MAP_PADDR_END {
                panic!("teardown: OOB paddr {:#x} at level {} idx {} entry {:#x}",
                    pv, level, i, entry);
            }
            teardown_table_dec_only(paddr, level - 1);
            free_pt_page(paddr);
        }
    }
}

fn duplicate_table(original_table_paddr: PAddr, level: usize) -> Result<PAddr, PageAllocError> {
    let orig_table = original_table_paddr.as_mut_ptr::<PageTableEntry>();
    let new_table_paddr = alloc_pt_page()?;
    let new_table = new_table_paddr.as_mut_ptr::<PageTableEntry>();

    // Bulk-copy the entire 4KB page table in one shot instead of zeroing
    // first and then copying entries one-by-one. Null entries are copied
    // as zeros, so skipped slots are already 0 (not present).
    unsafe {
        ptr::copy_nonoverlapping(orig_table, new_table, ENTRIES_PER_TABLE as usize);
    }

    debug_assert!(level > 0);

    if level == 1 {
        // Debug: detect if this PT page contains the PTE for 0xa0015e000
        // (the musl .rodata page with locale data that's zero in fork children).
        // We don't know the virtual address here, so log any non-trivial page tables.
        // The PTE at index (0x15e000 >> 12) & 0x1FF = 0x15E & 0x1FF = 0x15E.
        // But we need the full virtual address context to check... skip for now
        // and just verify the bulk copy worked.

        // Leaf page table (PTE level): fix up CoW entries.
        // The bulk copy already placed all entries. We only need to:
        // 1) Increment refcounts on user pages
        // 2) Clear WRITABLE on writable user pages (in BOTH parent and child)
        //
        // Batch null-check: scan 8 entries at a time (1 cache line).
        // If all 8 are zero, skip the batch entirely. For sparse PT pages
        // (~10 entries out of 512), this skips ~95% of iterations.
        for batch in 0..64isize {
            let base = batch * 8;
            let any = unsafe {
                *orig_table.offset(base) | *orig_table.offset(base + 1)
                | *orig_table.offset(base + 2) | *orig_table.offset(base + 3)
                | *orig_table.offset(base + 4) | *orig_table.offset(base + 5)
                | *orig_table.offset(base + 6) | *orig_table.offset(base + 7)
            };
            if any == 0 {
                continue;
            }

            for i in base..base + 8 {
                let entry = unsafe { *orig_table.offset(i) };
                let paddr = entry_paddr(entry);
                if paddr.is_null() {
                    continue;
                }
                let flags = entry_flags(entry);
                if flags & PageAttrs::USER.bits() == 0 {
                    continue;
                }
                crate::page_refcount::page_ref_inc(paddr);
                if flags & PageAttrs::WRITABLE.bits() != 0 {
                    let cow_entry = paddr.value() as u64 | (flags & !PageAttrs::WRITABLE.bits());
                    unsafe {
                        *orig_table.offset(i) = cow_entry;
                        *new_table.offset(i) = cow_entry;
                    }
                }
            }
        }
    } else {
        // Intermediate page table (PML4, PDPT, PD): fix up entries that
        // need recursion or CoW treatment (huge pages).
        for i in 0..ENTRIES_PER_TABLE {
            if level == 4 && i >= 0x80 {
                continue;
            }

            let entry = unsafe { *orig_table.offset(i) };
            let paddr = entry_paddr(entry);
            if paddr.is_null() {
                continue;
            }

            if level == 2 && is_huge_page_pde(entry) {
                let flags = entry_flags(entry);
                if flags & PageAttrs::USER.bits() == 0 {
                    continue;
                }
                crate::page_refcount::page_ref_inc_huge(paddr);
                if flags & PageAttrs::WRITABLE.bits() != 0 {
                    let cow_entry = paddr.value() as u64 | (flags & !PageAttrs::WRITABLE.bits());
                    unsafe {
                        *orig_table.offset(i) = cow_entry;
                        *new_table.offset(i) = cow_entry;
                    }
                }
            } else {
                let new_child_paddr = duplicate_table(paddr, level - 1)?;
                unsafe {
                    *new_table.offset(i) = new_child_paddr.value() as u64 | entry_flags(entry);
                }
            }
        }
    }

    Ok(new_table_paddr)
}

/// Physical address of the kernel's bootstrap PML4 (identity+kernel-half).
/// Safe to load as CR3: contains only kernel mappings, no user pages and no
/// PT pages that are subject to user-process teardown races.
pub fn kernel_pml4_paddr() -> PAddr {
    unsafe extern "C" {
        static __kernel_pml4: u8;
    }
    PAddr::new(unsafe { &__kernel_pml4 as *const u8 as usize })
}

/// Load the kernel bootstrap PML4 as CR3 on this CPU.  Used when switching
/// to a task that has no Vm (e.g. the idle thread): without this, CR3 keeps
/// pointing at the outgoing task's pml4.  If that pml4 later gets torn down,
/// the hardware walker can still traverse it and write A/D bits into freed
/// PT pages — corrupting the PT_PAGE_POOL cookie.
///
/// After this call, no page-walk on this CPU can reach user PT pages until
/// another task with a Vm is switched in.
pub fn load_kernel_pml4() {
    unsafe {
        // PCID=0 (bit 11..0 cleared), no-invalidate bit clear → full flush.
        x86::controlregs::cr3_write(kernel_pml4_paddr().value() as u64);
    }
}

fn allocate_pml4() -> Result<PAddr, PageAllocError> {
    unsafe extern "C" {
        static __kernel_pml4: u8;
    }

    let pml4 = alloc_pages(1, AllocPageFlags::KERNEL)?;

    // Map kernel pages.
    unsafe {
        let kernel_pml4 = PAddr::new(&__kernel_pml4 as *const u8 as usize).as_vaddr();
        pml4.as_mut_ptr::<u8>().write_bytes(0, PAGE_SIZE);
        ptr::copy_nonoverlapping::<u8>(kernel_pml4.as_ptr(), pml4.as_mut_ptr(), PAGE_SIZE);
    }

    // The kernel no longer access a virtual address around 0x0000_0000. Unmap
    // the area to catch bugs (especially NULL pointer dereferences in the
    // kernel).
    //
    // TODO: Is it able to unmap in boot.S before running bsp_early_init?
    unsafe {
        *pml4.as_mut_ptr::<PageTableEntry>().offset(0) = 0;
    }

    Ok(pml4)
}

pub struct PageTable {
    pml4: PAddr,
    /// Packed PCID + generation: bits [63:12] = generation, bits [11:0] = PCID.
    /// On context switch, if the stored generation is older than the global
    /// generation, the PCID's TLB entries are stale and must be flushed.
    /// AtomicU64 for interior mutability in switch() (called through &self).
    pcid_gen: core::sync::atomic::AtomicU64,
}

/// Global PCID state: bits [63:12] = generation, bits [11:0] = next PCID.
/// When PCID reaches 4095, generation increments and PCID resets to 1.
/// Stale TLB entries from a previous generation are flushed on first use.
static PCID_STATE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);

/// Bump the global PCID generation so that every process's next
/// `PageTable::switch()` sees a generation mismatch and does a full
/// CR3 reload (flushing all TLB entries for that PCID).  Used as
/// a non-IPI fallback when `tlb_shootdown` is called with IF=0.
pub fn bump_global_pcid_generation() {
    if !super::boot::PCID_SUPPORTED.load(core::sync::atomic::Ordering::Relaxed) {
        return;
    }
    // Atomically increment the generation (bits [63:12]) by 0x1000,
    // keeping the PCID counter (bits [11:0]) unchanged.  CAS loop
    // handles concurrent bumps.
    loop {
        let state = PCID_STATE.load(core::sync::atomic::Ordering::Relaxed);
        let generation = state & !0xFFF;
        let pcid_part = state & 0xFFF;
        let new_state = (generation + 0x1000) | pcid_part;
        if PCID_STATE.compare_exchange_weak(
            state, new_state,
            core::sync::atomic::Ordering::AcqRel,
            core::sync::atomic::Ordering::Relaxed,
        ).is_ok() {
            return;
        }
    }
}

/// Allocate a PCID+generation pair. Returns 0 if PCID not supported.
fn alloc_pcid() -> u64 {
    if !super::boot::PCID_SUPPORTED.load(core::sync::atomic::Ordering::Relaxed) {
        return 0;
    }
    loop {
        let state = PCID_STATE.load(core::sync::atomic::Ordering::Relaxed);
        let generation = state & !0xFFF; // upper bits = generation << 12
        let next = (state & 0xFFF) as u16;

        if next >= 4095 {
            // Exhausted: bump generation, reset to PCID 1.
            let new_state = (generation + 0x1000) | 1;
            if PCID_STATE.compare_exchange_weak(
                state, new_state,
                core::sync::atomic::Ordering::AcqRel,
                core::sync::atomic::Ordering::Relaxed,
            ).is_ok() {
                return new_state;
            }
            continue;
        }

        let new_state = generation | ((next + 1) as u64);
        if PCID_STATE.compare_exchange_weak(
            state, new_state,
            core::sync::atomic::Ordering::AcqRel,
            core::sync::atomic::Ordering::Relaxed,
        ).is_ok() {
            return generation | (next as u64);
        }
    }
}

impl PageTable {
    pub fn new() -> Result<PageTable, PageAllocError> {
        let pml4 = allocate_pml4()?;
        Ok(PageTable { pml4, pcid_gen: core::sync::atomic::AtomicU64::new(alloc_pcid()) })
    }

    /// Construct a PageTable from an existing PML4 for the sole purpose of
    /// running `teardown_forked_pages` / `teardown_ghost_pages` on it.
    /// Used by the deferred Vm teardown path: Vm::Drop under IF=0 stashes
    /// the pml4 and a safe-context drainer re-materializes a PageTable
    /// here to do the actual free.
    pub fn from_pml4_for_teardown(pml4: PAddr) -> PageTable {
        PageTable { pml4, pcid_gen: core::sync::atomic::AtomicU64::new(0) }
    }

    /// Zero out the pml4 field so the *original* PageTable's drop path
    /// won't double-free after we've handed the pml4 off to the deferred
    /// teardown list.
    pub fn clear_pml4_for_defer(&mut self) {
        self.pml4 = PAddr::new(0);
    }

    /// Returns the physical address of the PML4 (top-level page table).
    pub fn pml4(&self) -> PAddr {
        self.pml4
    }

    /// Extract the 12-bit PCID value from the packed pcid_gen.
    fn pcid(&self) -> u64 {
        self.pcid_gen.load(core::sync::atomic::Ordering::Relaxed) & 0xFFF
    }

    pub fn duplicate_from(original: &mut PageTable) -> Result<PageTable, PageAllocError> {
        let new_pml4 = duplicate_table_cow(original.pml4, 4)?;
        // PCID TLB coherency fix for fork:
        //
        // duplicate_table_cow cleared WRITABLE on the parent's PTEs. With
        // PCID, remote CPUs may cache stale WRITABLE entries for the parent's
        // old PCID. Allocate a FRESH PCID+gen for the parent so stale entries
        // (tagged with the old PCID) are never used again.
        //
        // Without PCID (pcid()==0): every CR3 write fully flushes the TLB.
        if original.pcid() != 0 {
            original.pcid_gen.store(alloc_pcid(), core::sync::atomic::Ordering::Relaxed);
        }
        // Flush the current CPU: load parent's PML4 WITHOUT bit 63 to
        // invalidate all entries for this PCID on this CPU.
        unsafe {
            let flush_cr3 = original.pml4.value() as u64 | original.pcid();
            x86::controlregs::cr3_write(flush_cr3);
        }
        Ok(PageTable { pml4: new_pml4, pcid_gen: core::sync::atomic::AtomicU64::new(alloc_pcid()) })
    }

    /// Ghost-fork: duplicate page table structure but skip all refcount
    /// operations. Returns (new PageTable, Vec of CoW-marked addresses).
    /// Parent must be blocked until child exec's/exits.
    pub fn duplicate_from_ghost(original: &PageTable) -> Result<(PageTable, Vec<usize>), PageAllocError> {
        let mut cow_addrs = Vec::new();
        let new_pml4 = duplicate_table_ghost(original.pml4, 4, 0, &mut cow_addrs)?;
        // Flush parent's TLB so read-only PTEs take effect.
        unsafe {
            let cr3_val = original.pml4.value() as u64 | original.pcid();
            x86::controlregs::cr3_write(cr3_val);
        }
        Ok((PageTable { pml4: new_pml4, pcid_gen: core::sync::atomic::AtomicU64::new(alloc_pcid()) }, cow_addrs))
    }

    pub fn switch(&self) {
        use core::sync::atomic::Ordering;
        let my_pcid_gen = self.pcid_gen.load(Ordering::Relaxed);
        let pcid = my_pcid_gen & 0xFFF;

        unsafe {
            if pcid == 0 {
                // No PCID: plain CR3 write (flushes entire TLB).
                x86::controlregs::cr3_write(self.pml4.value() as u64);
                return;
            }

            let my_gen = my_pcid_gen & !0xFFF;
            let global_gen = PCID_STATE.load(Ordering::Relaxed) & !0xFFF;

            if my_gen == global_gen {
                // Same generation: TLB entries for this PCID are valid.
                // Bit 63 = no-invalidate — preserves other PCIDs' entries.
                let cr3_val = self.pml4.value() as u64 | pcid | (1u64 << 63);
                x86::controlregs::cr3_write(cr3_val);
            } else {
                // Stale generation: this PCID was potentially reused by another
                // process. Flush by loading CR3 WITHOUT bit 63.
                let cr3_val = self.pml4.value() as u64 | pcid;
                x86::controlregs::cr3_write(cr3_val);
                // Update stored generation so subsequent switches are fast.
                self.pcid_gen.store(global_gen | pcid, Ordering::Relaxed);
            }
        }
    }

    /// Map a 2MB huge page at `vaddr` (must be 2MB-aligned).
    #[inline(always)]
    pub fn map_huge_user_page(&mut self, vaddr: UserVAddr, paddr: PAddr, prot_flags: i32) {
        let hv = vaddr.value();
        if hv <= 0xa000ad000 && hv + HUGE_PAGE_SIZE > 0xa000ad000 {
            log::warn!("MAP_HUGE {:#x}-{:#x} pa={:#x} COVERS 0xa000ad000!", hv, hv + HUGE_PAGE_SIZE, paddr.value());
        }
        debug_assert!(is_aligned(vaddr.value(), HUGE_PAGE_SIZE));
        debug_assert!(is_aligned(paddr.value(), HUGE_PAGE_SIZE));
        let mut attrs = PageAttrs::PRESENT | PageAttrs::USER | PageAttrs::HUGE_PAGE;
        if prot_flags & 2 != 0 { attrs |= PageAttrs::WRITABLE; }
        if prot_flags & 4 == 0 { attrs |= PageAttrs::NO_EXECUTE; }
        let pde_ptr = traverse_to_pd(self.pml4, vaddr, true,
            PageAttrs::PRESENT | PageAttrs::USER | PageAttrs::WRITABLE)
            .expect("failed to traverse to PD for huge page");
        unsafe {
            *pde_ptr = paddr.value() as u64 | attrs.bits();
        }
    }

    /// Unmap a 2MB huge page, returning the base physical address if it was mapped.
    pub fn unmap_huge_user_page(&mut self, vaddr: UserVAddr) -> Option<PAddr> {
        debug_assert!(is_aligned(vaddr.value(), HUGE_PAGE_SIZE));
        let pde_ptr = traverse_to_pd(self.pml4, vaddr, false,
            PageAttrs::PRESENT | PageAttrs::USER | PageAttrs::WRITABLE)?;
        let pde_val = unsafe { *pde_ptr };
        if !is_huge_page_pde(pde_val) {
            return None;
        }
        let paddr = entry_paddr(pde_val);
        unsafe { *pde_ptr = 0; }
        Some(paddr)
    }

    /// Check if `vaddr` falls within a 2MB huge page mapping.
    /// Returns `Some(pde_value)` if a huge PDE covers this address.
    pub fn is_huge_mapped(&self, vaddr: UserVAddr) -> Option<u64> {
        let pde_ptr = traverse_to_pd(self.pml4, vaddr, false,
            PageAttrs::PRESENT | PageAttrs::USER | PageAttrs::WRITABLE)?;
        let pde_val = unsafe { *pde_ptr };
        if is_huge_page_pde(pde_val) { Some(pde_val) } else { None }
    }

    /// Check if the PDE for `vaddr`'s 2MB region is empty (no PT or huge page).
    /// Returns true if the PDE is 0 or the PD doesn't exist yet.
    pub fn is_pde_empty(&self, vaddr: UserVAddr) -> bool {
        match traverse_to_pd(self.pml4, vaddr, false,
            PageAttrs::PRESENT | PageAttrs::USER | PageAttrs::WRITABLE) {
            Some(pde_ptr) => unsafe { *pde_ptr == 0u64 },
            None => true,
        }
    }

    /// Split a 2MB huge page into 512 × 4KB PTEs.
    /// Returns the base physical address on success.
    pub fn split_huge_page(&mut self, vaddr: UserVAddr) -> Option<PAddr> {
        split_huge_page(self.pml4, vaddr)
    }

    /// Update PDE flags for a 2MB huge page. Returns true if a huge page was found.
    pub fn update_huge_page_flags(&mut self, vaddr: UserVAddr, prot_flags: i32) -> bool {
        let pde_ptr = match traverse_to_pd(self.pml4, vaddr, false,
            PageAttrs::PRESENT | PageAttrs::USER | PageAttrs::WRITABLE) {
            Some(ptr) => ptr,
            None => return false,
        };
        let pde_val = unsafe { *pde_ptr };
        if !is_huge_page_pde(pde_val) {
            return false;
        }
        let paddr_bits = pde_val & 0x000f_ffff_ffff_f000;
        let mut attrs = PageAttrs::PRESENT | PageAttrs::USER | PageAttrs::HUGE_PAGE;
        if prot_flags & 2 != 0 { attrs |= PageAttrs::WRITABLE; }
        if prot_flags & 4 == 0 { attrs |= PageAttrs::NO_EXECUTE; }
        unsafe { *pde_ptr = paddr_bits | attrs.bits(); }
        true
    }

    #[inline(always)]
    pub fn map_user_page(&mut self, vaddr: UserVAddr, paddr: PAddr) {
        // Initialize CoW refcount for every user page mapping.
        crate::page_refcount::page_ref_init(paddr);
        self.map_page(
            vaddr,
            paddr,
            PageAttrs::PRESENT | PageAttrs::USER | PageAttrs::WRITABLE,
        );
    }

    /// Maps a user page with specific protection flags.
    /// `prot_flags` uses Linux mmap prot bits: PROT_READ=1, PROT_WRITE=2, PROT_EXEC=4.
    #[inline(always)]
    pub fn map_user_page_with_prot(&mut self, vaddr: UserVAddr, paddr: PAddr, prot_flags: i32) {
        crate::page_refcount::page_ref_init(paddr);
        let mut attrs = PageAttrs::PRESENT | PageAttrs::USER;
        if prot_flags & 2 != 0 {
            attrs |= PageAttrs::WRITABLE;
        }
        if prot_flags & 4 == 0 {
            attrs |= PageAttrs::NO_EXECUTE;
        }
        self.map_page(vaddr, paddr, attrs);
    }

    /// Maps a device memory page without touching refcounts.
    /// Used for PCI BAR mappings (framebuffer, etc.) where the physical
    /// address is not managed by the page allocator.
    #[inline(always)]
    pub fn map_device_page(&mut self, vaddr: UserVAddr, paddr: PAddr, prot_flags: i32) {
        let mut attrs = PageAttrs::PRESENT | PageAttrs::USER;
        if prot_flags & 2 != 0 {
            attrs |= PageAttrs::WRITABLE;
        }
        if prot_flags & 4 == 0 {
            attrs |= PageAttrs::NO_EXECUTE;
        }
        // Device memory (MMIO) must be uncacheable — writes must go directly
        // to the hardware, not to the CPU cache.  Without PCD, Xorg's fbdev
        // driver writes to a cached shadow and the display stays black.
        attrs |= PageAttrs::CACHE_DISABLE | PageAttrs::WRITE_THROUGH;
        self.map_page(vaddr, paddr, attrs);
    }

    /// Updates the flags of an already-mapped user page.
    /// Returns true if the page was mapped, false if not present.
    #[inline(always)]
    pub fn update_page_flags(&mut self, vaddr: UserVAddr, prot_flags: i32) -> bool {
        let entry_ptr = match traverse(self.pml4, vaddr, false,
            PageAttrs::PRESENT | PageAttrs::USER | PageAttrs::WRITABLE) {
            Some(ptr) => ptr,
            None => return false,
        };

        let entry = unsafe { *entry_ptr.as_ptr() };
        let paddr_bits = entry & 0x000f_ffff_ffff_f000;
        if paddr_bits == 0 {
            return false;
        }

        if prot_flags == 0 {
            // PROT_NONE: clear PRESENT but keep paddr so permissions can be
            // restored later without re-allocating the page.
            unsafe {
                *entry_ptr.as_ptr() = paddr_bits;
            }
            return true;
        }

        let mut attrs = PageAttrs::PRESENT | PageAttrs::USER;
        if prot_flags & 2 != 0 {
            attrs |= PageAttrs::WRITABLE;
        }
        if prot_flags & 4 == 0 {
            attrs |= PageAttrs::NO_EXECUTE;
        }

        unsafe {
            *entry_ptr.as_ptr() = paddr_bits | attrs.bits();
        }
        true
    }

    /// Batch-map contiguous user pages, traversing the page table hierarchy only
    /// once per leaf PT (2MB region) instead of once per page.
    /// Returns a u32 bitmask: bit i set = page i was mapped.
    /// Pages where the PTE was already occupied are NOT overwritten.
    #[inline(always)]
    pub fn batch_try_map_user_pages_with_prot(
        &mut self,
        start_vaddr: UserVAddr,
        paddrs: &[PAddr],
        count: usize,
        prot_flags: i32,
    ) -> u32 {
        let mut attrs = PageAttrs::PRESENT | PageAttrs::USER;
        if prot_flags & 2 != 0 { attrs |= PageAttrs::WRITABLE; }
        if prot_flags & 4 == 0 { attrs |= PageAttrs::NO_EXECUTE; }
        let attrs_bits = attrs.bits();

        let mut mapped: u32 = 0;
        let mut i = 0;

        while i < count {
            let vaddr_value = start_vaddr.value() + i * PAGE_SIZE;

            // Traverse PML4→PDPT→PD once to get the leaf PT base.
            let pt_base = match traverse_to_pt(
                self.pml4,
                UserVAddr::new(vaddr_value).unwrap(),
                true, attrs,
            ) {
                Some(ptr) => ptr,
                None => break,
            };

            // How many pages fit before crossing a 2MB PT boundary?
            let start_idx = leaf_pt_index(vaddr_value);
            let remaining_in_pt = ENTRIES_PER_TABLE as usize - start_idx;
            let batch_end = if i + remaining_in_pt < count { i + remaining_in_pt } else { count };

            // Write PTEs directly by index — no per-page traverse.
            let mut idx = start_idx;
            while i < batch_end {
                let entry_ptr = unsafe { pt_base.add(idx) };
                let new_val = paddrs[i].value() as u64 | attrs_bits;
                let atom = unsafe { &*(entry_ptr as *const core::sync::atomic::AtomicU64) };
                if atom.compare_exchange(0, new_val,
                    core::sync::atomic::Ordering::AcqRel,
                    core::sync::atomic::Ordering::Relaxed).is_ok()
                {
                    mapped |= 1 << i;
                }
                idx += 1;
                i += 1;
            }
        }

        mapped
    }

    /// Unmaps a user page, returning the physical address if it was mapped.
    #[inline(always)]
    /// Look up the physical address for a mapped user page (read-only).
    /// For huge pages, returns base_paddr + offset within the 2MB page.
    pub fn lookup_paddr(&self, vaddr: UserVAddr) -> Option<PAddr> {
        // First try normal 4KB lookup (read-only or writable).
        if let Some(entry_ptr) = traverse(self.pml4, vaddr, false,
            PageAttrs::PRESENT | PageAttrs::USER) {
            let entry = unsafe { *entry_ptr.as_ptr() };
            let paddr = entry_paddr(entry);
            if !paddr.is_null() {
                return Some(paddr);
            }
        }
        // traverse returns None when a huge page PDE is encountered.
        // Check for a 2MB huge page mapping.
        if let Some(pde_val) = self.is_huge_mapped(vaddr) {
            let base = entry_paddr(pde_val);
            let offset = vaddr.value() & (HUGE_PAGE_SIZE - 1);
            return Some(PAddr::new(base.value() + offset));
        }
        None
    }

    /// Look up the raw PTE value for a mapped user page.
    /// Returns the full u64 PTE entry including flags.
    /// Used by audit-vm to check permission bits.
    pub fn lookup_pte_entry(&self, vaddr: UserVAddr) -> Option<u64> {
        if let Some(entry_ptr) = traverse(self.pml4, vaddr, false,
            PageAttrs::PRESENT | PageAttrs::USER | PageAttrs::WRITABLE) {
            let entry = unsafe { *entry_ptr.as_ptr() };
            if entry != 0 { return Some(entry); }
        }
        None
    }

    pub fn unmap_user_page(&mut self, vaddr: UserVAddr) -> Option<PAddr> {
        let entry_ptr = match traverse(self.pml4, vaddr, false,
            PageAttrs::PRESENT | PageAttrs::USER | PageAttrs::WRITABLE) {
            Some(ptr) => ptr,
            None => return None,
        };

        let entry = unsafe { *entry_ptr.as_ptr() };
        let paddr = entry_paddr(entry);
        if paddr.is_null() {
            return None;
        }

        // Clear the PTE.
        unsafe {
            *entry_ptr.as_ptr() = 0;
        }
        Some(paddr)
    }

    /// Walk all user page table entries, decrement refcounts, free pages whose
    /// refcount reaches zero, and free intermediate page table pages.
    /// Called when a process's address space is destroyed (exec or exit).
    pub fn teardown_user_pages(&mut self) {
        // Invalidate stale TLB entries on all CPUs before freeing PT pages.
        // Other CPUs may have cached translations from this address space; if
        // they remain after we hand the PT pages back to the page allocator,
        // a page-walk-driven accessed/dirty bit update could corrupt the
        // newly-reused page (e.g. zeroing out the PT_PAGE_MAGIC cookie).
        flush_tlb_for_teardown();
        teardown_table(self.pml4, 4);
    }

    /// Decrement refcounts on all user pages and free intermediate page table
    /// pages, but NEVER free data pages even if their refcount reaches zero.
    /// This is safe for fork+exec: the forked page table's refcount increments
    /// are reversed so the parent's pages return to their correct refcount,
    /// avoiding unnecessary full CoW copies (refcount > 1 → must copy).
    /// The PML4 page itself is also freed.
    pub fn teardown_forked_pages(&mut self) {
        if self.pml4.is_null() {
            return;
        }
        flush_tlb_for_teardown();
        teardown_table_dec_only(self.pml4, 4);
        // Free the PML4 page itself and prevent double-free.
        let pml4 = self.pml4;
        self.pml4 = PAddr::new(0);
        crate::page_allocator::free_pages(pml4, 1);
    }

    /// Teardown a ghost-forked page table: free page table pages and any
    /// CoW-copied data pages, but don't touch parent-owned page refcounts.
    pub fn teardown_ghost_pages(&mut self) {
        if self.pml4.is_null() {
            return;
        }
        flush_tlb_for_teardown();
        teardown_table_ghost(self.pml4, 4);
        let pml4 = self.pml4;
        self.pml4 = PAddr::new(0);
        crate::page_allocator::free_pages(pml4, 1);
    }

    /// Restore WRITABLE on PTEs listed in `addrs` (collected during ghost fork).
    /// O(N) where N = number of writable pages (~200), not O(all_PTEs) (~10K).
    pub fn restore_writable_from(&mut self, addrs: &[usize]) {
        restore_writable_from_list(self.pml4, addrs);
    }

    /// Try to map a page. Returns `true` if mapped, `false` if already mapped.
    /// Allocates intermediate page tables as needed.
    #[inline(always)]
    pub fn try_map_user_page_with_prot(
        &mut self,
        vaddr: UserVAddr,
        paddr: PAddr,
        prot_flags: i32,
    ) -> bool {
        let mut attrs = PageAttrs::PRESENT | PageAttrs::USER;
        if prot_flags & 2 != 0 {
            attrs |= PageAttrs::WRITABLE;
        }
        if prot_flags & 4 == 0 {
            attrs |= PageAttrs::NO_EXECUTE;
        }
        // traverse returns None when a 2MB huge PDE covers this address.
        // Cannot map a 4KB PTE inside a huge page — return false (not mapped).
        let mut entry = match traverse(self.pml4, vaddr, true, attrs) {
            Some(e) => e,
            None => return false,
        };
        // Atomic compare-and-swap: only write the PTE if it's currently 0.
        // Without this, two CPUs handling a demand fault on the same virtual
        // address could both read 0 and both write, with the second write
        // silently replacing the first CPU's page — leaving the first CPU
        // executing from a page no longer in the page table.
        unsafe {
            let entry_ptr = entry.as_ptr() as *const core::sync::atomic::AtomicU64;
            let new_val = paddr.value() as u64 | attrs.bits();
            (*entry_ptr)
                .compare_exchange(0, new_val, core::sync::atomic::Ordering::AcqRel, core::sync::atomic::Ordering::Relaxed)
                .is_ok()
        }
    }

    /// Flushes the TLB for a specific virtual address on all online CPUs.
    ///
    /// On SMP systems this broadcasts a TLB-shootdown IPI so that every CPU
    /// using this address space invalidates its local TLB entry.  The local
    /// `invlpg` is issued before the IPI is sent.
    #[inline(always)]
    pub fn flush_tlb(&self, vaddr: UserVAddr) {
        super::apic::tlb_shootdown(vaddr.value());
    }

    /// Flushes the TLB for a specific virtual address on the LOCAL CPU only.
    ///
    /// Used by bulk operations (e.g. sys_munmap) that flush all pages locally
    /// with individual invlpg calls and then send a single remote IPI
    /// via `flush_tlb_remote()` to cover all of them at once.
    #[inline(always)]
    pub fn flush_tlb_local(&self, vaddr: UserVAddr) {
        unsafe {
            core::arch::asm!("invlpg [{}]", in(reg) vaddr.value(),
                options(nostack, preserves_flags));
        }
    }

    /// Sends ONE IPI to all other CPUs telling them to invalidate ALL PCIDs.
    ///
    /// Call after a batch of `flush_tlb_local` calls to flush remote CPUs
    /// with a single IPI round-trip. Uses INVPCID type=3 (or CR4.PCIDE
    /// toggle as fallback) so that stale entries tagged with this process's
    /// PCID are invalidated on CPUs currently scheduling a different
    /// process — without this, those entries remain dormant in the remote
    /// TLB until the PCID is reused, allowing a stale write into a
    /// freed-then-reissued page (often a kernel slab page).
    #[inline(always)]
    pub fn flush_tlb_remote(&self) {
        super::apic::tlb_remote_flush_all_pcids();
    }

    /// Flushes the entire TLB by reloading CR3.
    pub fn flush_tlb_all(&self) {
        // Reload CR3 WITHOUT bit 63 to flush entries for this PCID.
        unsafe {
            let cr3_val = self.pml4.value() as u64 | self.pcid();
            x86::controlregs::cr3_write(cr3_val);
        }
    }

    #[inline(always)]
    fn map_page(&mut self, vaddr: UserVAddr, paddr: PAddr, attrs: PageAttrs) {
        debug_assert!(is_aligned(vaddr.value(), PAGE_SIZE));
        let attrs_bits = attrs.bits();
        let mut entry = traverse(self.pml4, vaddr, true, attrs).unwrap();
        unsafe {
            *entry.as_mut() = paddr.value() as u64 | attrs_bits;
        }
    }

}

// TODO: Add Drop for PageTable to free the PML4 page. Currently the PML4
// (4 KB per process) is leaked. Intermediate tables are freed by
// teardown_user_pages() via Vm::Drop. The PML4 free requires verifying
// it's not the boot page table and not currently loaded in CR3.
