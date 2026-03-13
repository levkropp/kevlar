// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! ARM64 4-level page table management (4KB granule, 48-bit VA).
//!
//! Level 0 (PGD): bits [47:39], 512 entries
//! Level 1 (PUD): bits [38:30], 512 entries, can use 1GB blocks
//! Level 2 (PMD): bits [29:21], 512 entries, can use 2MB blocks
//! Level 3 (PTE): bits [20:12], 512 entries, 4KB pages
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

// ARM64 descriptor bits.
const DESC_VALID: u64 = 1 << 0;
const DESC_TABLE: u64 = 1 << 1; // Table descriptor (levels 0-2)
const DESC_PAGE: u64 = 1 << 1;  // Page descriptor (level 3)
// Lower attributes.
const ATTR_IDX_NORMAL: u64 = 1 << 2; // MAIR index 1 = Normal WB
// AP[2:1] (bits [7:6]):
//   00 = EL1 RW, EL0 no access
//   01 = EL1 RW, EL0 RW
//   10 = EL1 RO, EL0 no access
//   11 = EL1 RO, EL0 RO
const ATTR_AP_USER: u64 = 1 << 6;    // AP[1] = EL0 accessible
const ATTR_AP_RO: u64 = 1 << 7;      // AP[2] = Read-only
const ATTR_SH_ISH: u64 = 3 << 8;     // Inner Shareable
const ATTR_AF: u64 = 1 << 10;        // Access Flag
// Upper attributes.
const ATTR_PXN: u64 = 1 << 53;       // Privileged Execute Never
const ATTR_UXN: u64 = 1 << 54;       // Unprivileged Execute Never (XN)

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

fn entry_paddr(entry: PageTableEntry) -> PAddr {
    PAddr::new((entry & 0x0000_FFFF_FFFF_F000) as usize)
}

fn entry_flags(entry: PageTableEntry) -> u64 {
    entry & !0x0000_FFFF_FFFF_F000
}

fn nth_level_table_index(vaddr: UserVAddr, level: usize) -> isize {
    ((vaddr.value() >> ((level * 9) + 12)) & 0x1ff) as isize
}

/// Walk the page table to find or allocate the level-3 PTE for `vaddr`.
fn traverse(
    pgd: PAddr,
    vaddr: UserVAddr,
    allocate: bool,
) -> Option<NonNull<PageTableEntry>> {
    debug_assert!(is_aligned(vaddr.value(), PAGE_SIZE));
    let mut table = pgd.as_mut_ptr::<PageTableEntry>();

    // Walk levels 3 → 2 → 1 (PGD → PUD → PMD), stopping before level 0 (PTE).
    for level in (1..=3).rev() {
        let index = nth_level_table_index(vaddr, level);
        let entry = unsafe { table.offset(index) };
        let mut table_paddr = entry_paddr(unsafe { *entry });
        if table_paddr.value() == 0 {
            if !allocate {
                return None;
            }
            let new_table =
                alloc_pages(1, AllocPageFlags::KERNEL).expect("failed to allocate page table");
            unsafe {
                new_table.as_mut_ptr::<u8>().write_bytes(0, PAGE_SIZE);
                *entry = new_table.value() as u64 | DESC_VALID | DESC_TABLE;
            }
            table_paddr = new_table;
        }
        table = table_paddr.as_mut_ptr::<PageTableEntry>();
    }

    // Now `table` points to the level-0 (PTE) table.
    unsafe {
        Some(NonNull::new_unchecked(
            table.offset(nth_level_table_index(vaddr, 0)),
        ))
    }
}

fn duplicate_table(original_table_paddr: PAddr, level: usize) -> Result<PAddr, PageAllocError> {
    let orig_table = original_table_paddr.as_ptr::<PageTableEntry>();
    let new_table_paddr = alloc_pages(1, AllocPageFlags::KERNEL)?;
    let new_table = new_table_paddr.as_mut_ptr::<PageTableEntry>();

    debug_assert!(level > 0);
    for i in 0..ENTRIES_PER_TABLE {
        let entry = unsafe { *orig_table.offset(i) };
        let paddr = entry_paddr(entry);

        if paddr.is_null() {
            continue;
        }

        let new_paddr = if level == 1 {
            // Copy a physical page.
            let new_paddr = alloc_pages(1, AllocPageFlags::KERNEL)?;
            unsafe {
                ptr::copy_nonoverlapping::<u8>(paddr.as_ptr(), new_paddr.as_mut_ptr(), PAGE_SIZE);
            }
            new_paddr
        } else {
            duplicate_table(paddr, level - 1)?
        };

        unsafe {
            *new_table.offset(i) = new_paddr.value() as u64 | entry_flags(entry);
        }
    }

    Ok(new_table_paddr)
}

fn allocate_pgd() -> Result<PAddr, PageAllocError> {
    // Allocate a fresh user PGD (TTBR0). No kernel entries needed since
    // kernel uses TTBR1 exclusively.
    let pgd = alloc_pages(1, AllocPageFlags::KERNEL)?;
    unsafe {
        pgd.as_mut_ptr::<u8>().write_bytes(0, PAGE_SIZE);
    }
    Ok(pgd)
}

/// Translate Linux mmap prot flags to ARM64 PTE attributes for a user page.
fn prot_to_attrs(prot_flags: i32) -> u64 {
    let mut attrs = DESC_VALID | DESC_PAGE | ATTR_IDX_NORMAL | ATTR_SH_ISH | ATTR_AF | ATTR_AP_USER;

    if prot_flags & 2 == 0 {
        // No PROT_WRITE → read-only
        attrs |= ATTR_AP_RO;
    }
    if prot_flags & 4 == 0 {
        // No PROT_EXEC → execute never
        attrs |= ATTR_UXN;
    }
    // Always set PXN for user pages — kernel should never execute user memory.
    attrs |= ATTR_PXN;

    attrs
}

pub struct PageTable {
    pgd: PAddr,
}

impl PageTable {
    pub fn new() -> Result<PageTable, PageAllocError> {
        let pgd = allocate_pgd()?;
        Ok(PageTable { pgd })
    }

    pub fn duplicate_from(original: &PageTable) -> Result<PageTable, PageAllocError> {
        Ok(PageTable {
            pgd: duplicate_table(original.pgd, 4)?,
        })
    }

    pub fn switch(&self) {
        unsafe {
            core::arch::asm!(
                "msr ttbr0_el1, {}",
                "tlbi vmalle1",
                "dsb sy",
                "isb",
                in(reg) self.pgd.value() as u64,
            );
        }
    }

    pub fn map_user_page(&mut self, vaddr: UserVAddr, paddr: PAddr) {
        // Default: RW, no exec.
        let attrs = DESC_VALID | DESC_PAGE | ATTR_IDX_NORMAL | ATTR_SH_ISH
            | ATTR_AF | ATTR_AP_USER | ATTR_PXN | ATTR_UXN;
        self.map_page(vaddr, paddr, attrs);
    }

    pub fn map_user_page_with_prot(&mut self, vaddr: UserVAddr, paddr: PAddr, prot_flags: i32) {
        let attrs = prot_to_attrs(prot_flags);
        self.map_page(vaddr, paddr, attrs);
    }

    /// Maps a page only if the entry is not already present.
    /// Returns `true` if the page was mapped, `false` if already mapped.
    pub fn try_map_user_page_with_prot(
        &mut self,
        vaddr: UserVAddr,
        paddr: PAddr,
        prot_flags: i32,
    ) -> bool {
        let attrs = prot_to_attrs(prot_flags);
        let entry_ptr = match traverse(self.pgd, vaddr, true) {
            Some(ptr) => ptr,
            None => return false,
        };
        unsafe {
            if *entry_ptr.as_ptr() != 0 {
                return false; // already mapped
            }
            *entry_ptr.as_ptr() = paddr.value() as u64 | attrs;
            true
        }
    }

    pub fn update_page_flags(&mut self, vaddr: UserVAddr, prot_flags: i32) -> bool {
        let entry_ptr = match traverse(self.pgd, vaddr, false) {
            Some(ptr) => ptr,
            None => return false,
        };

        let entry = unsafe { *entry_ptr.as_ptr() };
        let paddr_bits = entry & 0x0000_FFFF_FFFF_F000;
        if paddr_bits == 0 {
            return false;
        }

        let attrs = prot_to_attrs(prot_flags);
        unsafe {
            *entry_ptr.as_ptr() = paddr_bits | attrs;
        }
        true
    }

    pub fn unmap_user_page(&mut self, vaddr: UserVAddr) -> Option<PAddr> {
        let entry_ptr = match traverse(self.pgd, vaddr, false) {
            Some(ptr) => ptr,
            None => return None,
        };

        let entry = unsafe { *entry_ptr.as_ptr() };
        let paddr = entry_paddr(entry);
        if paddr.is_null() {
            return None;
        }

        unsafe {
            *entry_ptr.as_ptr() = 0;
        }
        Some(paddr)
    }

    pub fn flush_tlb(&self, vaddr: UserVAddr) {
        unsafe {
            let addr = vaddr.value() >> 12;
            core::arch::asm!(
                "tlbi vale1, {}",
                "dsb ish",
                "isb",
                in(reg) addr,
            );
        }
    }

    /// On ARM64, `tlbi vale1` with `dsb ish` already broadcasts to all CPUs
    /// in the inner shareable domain — there is no separate "local only" step.
    /// This is an alias of `flush_tlb` for interface parity with x86_64.
    #[inline(always)]
    pub fn flush_tlb_local(&self, vaddr: UserVAddr) {
        self.flush_tlb(vaddr);
    }

    /// On ARM64, TLB invalidation is broadcast automatically; a separate
    /// "remote flush" IPI is not needed.  This is a no-op for interface parity.
    #[inline(always)]
    pub fn flush_tlb_remote(&self) {}

    pub fn flush_tlb_all(&self) {
        unsafe {
            core::arch::asm!(
                "tlbi vmalle1",
                "dsb sy",
                "isb",
            );
        }
    }

    fn map_page(&mut self, vaddr: UserVAddr, paddr: PAddr, attrs: u64) {
        debug_assert!(is_aligned(vaddr.value(), PAGE_SIZE));
        let mut entry = traverse(self.pgd, vaddr, true).unwrap();
        unsafe {
            *entry.as_mut() = paddr.value() as u64 | attrs;
        }
    }
}
