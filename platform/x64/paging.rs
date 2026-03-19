// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use super::PAGE_SIZE;
use crate::address::{PAddr, UserVAddr};
use crate::page_allocator::{alloc_pages, AllocPageFlags, PageAllocError};
use bitflags::bitflags;
use core::{
    debug_assert,
    ptr::{self, NonNull},
};
use kevlar_utils::alignment::is_aligned;

const ENTRIES_PER_TABLE: isize = 512;
type PageTableEntry = u64;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct PageAttrs: u64 {
        const PRESENT = 1 << 0;
        const WRITABLE = 1 << 1;
        const USER = 1 << 2;
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
    PAddr::new((entry & 0x7ffffffffffff000) as usize)
}

#[inline(always)]
fn entry_flags(entry: PageTableEntry) -> PageTableEntry {
    entry & !0x7ffffffffffff000
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

/// Recursively walk a page table, decrementing refcounts on user pages
/// and freeing intermediate page table pages. Called from `teardown_user_pages`.
fn teardown_table(table_paddr: PAddr, level: usize) {
    let table = table_paddr.as_mut_ptr::<PageTableEntry>();

    for i in 0..ENTRIES_PER_TABLE {
        let entry = unsafe { *table.offset(i) };
        let paddr = entry_paddr(entry);

        if paddr.is_null() {
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
            crate::page_allocator::free_pages(paddr, 1);
        }
    }
}

fn duplicate_table(original_table_paddr: PAddr, level: usize) -> Result<PAddr, PageAllocError> {
    let orig_table = original_table_paddr.as_mut_ptr::<PageTableEntry>();
    let new_table_paddr = alloc_pages(1, AllocPageFlags::KERNEL)?;
    let new_table = new_table_paddr.as_mut_ptr::<PageTableEntry>();

    // Bulk-copy the entire 4KB page table in one shot instead of zeroing
    // first and then copying entries one-by-one. Null entries are copied
    // as zeros, so skipped slots are already 0 (not present).
    unsafe {
        ptr::copy_nonoverlapping(orig_table, new_table, ENTRIES_PER_TABLE as usize);
    }

    debug_assert!(level > 0);

    if level == 1 {
        // Leaf page table (PTE level): fix up CoW entries.
        // The bulk copy already placed all entries. We only need to:
        // 1) Increment refcounts on user pages
        // 2) Clear WRITABLE on writable user pages (in BOTH parent and child)
        for i in 0..ENTRIES_PER_TABLE {
            let entry = unsafe { *orig_table.offset(i) };
            let paddr = entry_paddr(entry);

            if paddr.is_null() {
                continue;
            }

            let flags = entry_flags(entry);
            if flags & PageAttrs::USER.bits() == 0 {
                continue; // Kernel page: already copied correctly.
            }

            // Share the physical page between parent and child.
            crate::page_refcount::page_ref_inc(paddr);

            if flags & PageAttrs::WRITABLE.bits() != 0 {
                // CoW: clear WRITABLE in BOTH parent and child PTEs.
                let cow_entry = paddr.value() as u64 | (flags & !PageAttrs::WRITABLE.bits());
                unsafe {
                    *orig_table.offset(i) = cow_entry;
                    *new_table.offset(i) = cow_entry;
                }
            }
            // Read-only entries: already correct from bulk copy.
        }
    } else {
        // Intermediate page table (PML4, PDPT, PD): fix up entries that
        // need recursion or CoW treatment (huge pages).
        for i in 0..ENTRIES_PER_TABLE {
            if level == 4 && i >= 0x80 {
                // Kernel entries: already correct from bulk copy (shared).
                continue;
            }

            let entry = unsafe { *orig_table.offset(i) };
            let paddr = entry_paddr(entry);

            if paddr.is_null() {
                continue;
            }

            if level == 2 && is_huge_page_pde(entry) {
                // 2MB huge page PDE: treat as a leaf for CoW purposes.
                let flags = entry_flags(entry);
                if flags & PageAttrs::USER.bits() == 0 {
                    continue; // Already correct from bulk copy.
                }

                // Share the 2MB page: increment refcount on ALL 512 sub-PFNs.
                crate::page_refcount::page_ref_inc_huge(paddr);

                if flags & PageAttrs::WRITABLE.bits() != 0 {
                    // CoW: clear WRITABLE in both parent and child PDEs.
                    let cow_entry = paddr.value() as u64 | (flags & !PageAttrs::WRITABLE.bits());
                    unsafe {
                        *orig_table.offset(i) = cow_entry;
                        *new_table.offset(i) = cow_entry;
                    }
                }
                // Read-only huge pages: already correct from bulk copy.
            } else {
                // Intermediate entry: recurse to duplicate the child table.
                let new_child_paddr = duplicate_table(paddr, level - 1)?;
                // Replace the child table paddr, preserving flags from bulk copy.
                unsafe {
                    *new_table.offset(i) = new_child_paddr.value() as u64 | entry_flags(entry);
                }
            }
        }
    }

    Ok(new_table_paddr)
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
    /// Process Context ID (0-4095) for TLB tagging.
    /// Each address space gets a unique PCID so context switches don't
    /// flush the entire TLB — only entries for the target PCID are used.
    pcid: u16,
}

/// Next PCID to allocate. Wraps at 4095 (12-bit field). 0 = kernel.
static NEXT_PCID: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(1);

fn alloc_pcid() -> u16 {
    if !super::boot::PCID_SUPPORTED.load(core::sync::atomic::Ordering::Relaxed) {
        return 0; // No PCID support — use 0 (no PCID bits in CR3)
    }
    loop {
        let pcid = NEXT_PCID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let pcid = pcid & 0xFFF; // wrap to 12 bits
        if pcid != 0 { return pcid; } // 0 reserved for kernel
    }
}

impl PageTable {
    pub fn new() -> Result<PageTable, PageAllocError> {
        let pml4 = allocate_pml4()?;
        Ok(PageTable { pml4, pcid: alloc_pcid() })
    }

    /// Returns the physical address of the PML4 (top-level page table).
    pub fn pml4(&self) -> PAddr {
        self.pml4
    }

    pub fn duplicate_from(original: &PageTable) -> Result<PageTable, PageAllocError> {
        let new_pml4 = duplicate_table_cow(original.pml4, 4)?;
        // Flush user TLB entries for parent's PCID (CoW correctness).
        unsafe {
            let cr3_val = original.pml4.value() as u64 | (original.pcid as u64);
            x86::controlregs::cr3_write(cr3_val);
        }
        Ok(PageTable { pml4: new_pml4, pcid: alloc_pcid() })
    }

    pub fn switch(&self) {
        unsafe {
            if self.pcid != 0 {
                // PCID enabled: bits [11:0] = PCID, bit 63 = no-invalidate.
                let cr3_val = self.pml4.value() as u64 | (self.pcid as u64) | (1u64 << 63);
                x86::controlregs::cr3_write(cr3_val);
            } else {
                // No PCID: plain CR3 write (flushes entire TLB).
                x86::controlregs::cr3_write(self.pml4.value() as u64);
            }
        }
    }

    /// Map a 2MB huge page at `vaddr` (must be 2MB-aligned).
    #[inline(always)]
    pub fn map_huge_user_page(&mut self, vaddr: UserVAddr, paddr: PAddr, prot_flags: i32) {
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
        let paddr_bits = pde_val & 0x7ffffffffffff000;
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
            // PROT_WRITE
            attrs |= PageAttrs::WRITABLE;
        }
        if prot_flags & 4 == 0 {
            // No PROT_EXEC → set NX bit
            attrs |= PageAttrs::NO_EXECUTE;
        }
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
        let paddr_bits = entry & 0x7ffffffffffff000;
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
                let entry_val = unsafe { *entry_ptr };
                if entry_val == 0 {
                    unsafe { *entry_ptr = paddrs[i].value() as u64 | attrs_bits; }
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
        // First try normal 4KB lookup.
        if let Some(entry_ptr) = traverse(self.pml4, vaddr, false,
            PageAttrs::PRESENT | PageAttrs::USER | PageAttrs::WRITABLE) {
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
        teardown_table(self.pml4, 4);
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
        unsafe {
            if *entry.as_ptr() != 0 {
                return false; // already mapped
            }
            *entry.as_mut() = paddr.value() as u64 | attrs.bits();
            true
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

    /// Sends ONE IPI to all other CPUs telling them to reload CR3 (full TLB flush).
    ///
    /// Call after a batch of `flush_tlb_local` calls to flush remote CPUs
    /// with a single IPI round-trip instead of one per page.
    #[inline(always)]
    pub fn flush_tlb_remote(&self) {
        super::apic::tlb_remote_full_flush();
    }

    /// Flushes the entire TLB by reloading CR3.
    pub fn flush_tlb_all(&self) {
        // Reload CR3 WITHOUT bit 63 to flush entries.
        unsafe {
            let cr3_val = self.pml4.value() as u64 | (self.pcid as u64);
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
