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
        const NO_EXECUTE = 1 << 63;
    }
}

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

#[inline(always)]
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
    attrs: PageAttrs,
) -> Option<NonNull<PageTableEntry>> {
    debug_assert!(is_aligned(vaddr.value(), PAGE_SIZE));
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
                *entry = new_table.value() as u64 | attrs.bits()
            };
            table = new_table.as_mut_ptr::<PageTableEntry>();
        } else {
            // Only write if value changed to avoid unnecessary cache line dirtying.
            let expected = table_paddr.value() as u64 | attrs.bits();
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
    attrs: PageAttrs,
) -> Option<*mut PageTableEntry> {
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
            unsafe { *entry = new_table.value() as u64 | attrs.bits() };
            table = new_table.as_mut_ptr::<PageTableEntry>();
        } else {
            let expected = table_paddr.value() as u64 | attrs.bits();
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

/// Duplicates entires (and referenced memory pages if `level == 1`) in the
/// nth-level page table. Returns the newly created copy of the page table.
///
/// fork(2) uses this funciton to duplicate the memory space.
fn duplicate_table(original_table_paddr: PAddr, level: usize) -> Result<PAddr, PageAllocError> {
    let orig_table = original_table_paddr.as_mut_ptr::<PageTableEntry>();
    let new_table_paddr = alloc_pages(1, AllocPageFlags::KERNEL)?;
    let new_table = new_table_paddr.as_mut_ptr::<PageTableEntry>();

    // Zero the new table — skipped entries must be 0 (not present),
    // not garbage from a previous page allocation.
    unsafe { new_table.write_bytes(0, ENTRIES_PER_TABLE as usize); }

    debug_assert!(level > 0);
    for i in 0..ENTRIES_PER_TABLE {
        let entry = unsafe { *orig_table.offset(i) };
        let paddr = entry_paddr(entry);

        if paddr.is_null() {
            continue;
        }

        if level == 1 {
            // Leaf page table (PTE level): implement Copy-on-Write.
            let flags = entry_flags(entry);
            let is_user = flags & PageAttrs::USER.bits() != 0;
            let is_writable = flags & PageAttrs::WRITABLE.bits() != 0;

            if !is_user {
                // Kernel page: copy PTE as-is (shared kernel mappings).
                unsafe { *new_table.offset(i) = entry; }
                continue;
            }

            // Share the physical page between parent and child.
            // Increment the page's reference count.
            crate::page_refcount::page_ref_inc(paddr);

            if is_writable {
                // CoW: clear WRITABLE in BOTH parent and child PTEs.
                // When either process writes, the page fault handler will
                // allocate a new page and copy (see handle_cow_fault).
                let cow_flags = flags & !PageAttrs::WRITABLE.bits();
                unsafe {
                    // Update parent's PTE to read-only.
                    *orig_table.offset(i) = paddr.value() as u64 | cow_flags;
                    // Child gets same page, same read-only flags.
                    *new_table.offset(i) = paddr.value() as u64 | cow_flags;
                }
            } else {
                // Already read-only (code, rodata): share directly.
                unsafe { *new_table.offset(i) = entry; }
            }
        } else {
            // Intermediate page table (PML4, PDPT, PD): recurse.
            let new_paddr = if level == 4 && i >= 0x80 {
                // Kernel page table entries are immutable.
                entry_paddr(entry)
            } else {
                duplicate_table(paddr, level - 1)?
            };
            unsafe {
                *new_table.offset(i) = new_paddr.value() as u64 | entry_flags(entry);
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
}

impl PageTable {
    pub fn new() -> Result<PageTable, PageAllocError> {
        let pml4 = allocate_pml4()?;
        Ok(PageTable { pml4 })
    }

    pub fn duplicate_from(original: &PageTable) -> Result<PageTable, PageAllocError> {
        let new_pml4 = duplicate_table(original.pml4, 4)?;
        // CoW fork marked parent's writable PTEs as read-only. Flush the
        // TLB so the parent doesn't bypass CoW via stale writable entries.
        // A write through a stale entry would modify the shared physical
        // page, violating fork's independent-copy semantics.
        unsafe { x86::tlb::flush_all(); }
        Ok(PageTable { pml4: new_pml4 })
    }

    pub fn switch(&self) {
        unsafe {
            x86::controlregs::cr3_write(self.pml4.value() as u64);
        }
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
    pub fn lookup_paddr(&self, vaddr: UserVAddr) -> Option<PAddr> {
        let entry_ptr = match traverse(self.pml4, vaddr, false,
            PageAttrs::PRESENT | PageAttrs::USER | PageAttrs::WRITABLE) {
            Some(ptr) => ptr,
            None => return None,
        };
        let entry = unsafe { *entry_ptr.as_ptr() };
        let paddr = entry_paddr(entry);
        if paddr.is_null() { None } else { Some(paddr) }
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
        let mut entry = traverse(self.pml4, vaddr, true, attrs).unwrap();
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
        self.switch();
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
