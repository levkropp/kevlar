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
use alloc::vec::Vec;
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
const ATTR_IDX_DEVICE: u64 = 0 << 2; // MAIR index 0 = Device-nGnRnE (MMIO)
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
// nG (bit 11): non-Global.  Set on user PTEs so they're ASID-tagged in the
// TLB (required for TCR.AS=1 ASID fast-path switch).  Kernel PTEs leave
// this clear — kernel mappings are global across all ASIDs.
const ATTR_NG: u64 = 1 << 11;
// Upper attributes.
const ATTR_PXN: u64 = 1 << 53;       // Privileged Execute Never
const ATTR_UXN: u64 = 1 << 54;       // Unprivileged Execute Never (XN)

// Software-reserved PTE bits (ARMv8 descriptors allocate bits 55-58 to the OS).
/// "Was writable before ghost-fork CoW."  Set alongside ATTR_AP_RO on a page
/// that the ghost child CoW-marked; `restore_writable_from_list` uses it to
/// clear ATTR_AP_RO on the parent's PTEs after the child exits / execs.
const PTE_WAS_WRITABLE: u64 = 1 << 55;
/// "This descriptor points at a shared leaf-PT page."  Set on a level-2
/// (PMD) descriptor whose target level-1 (leaf) PT is shared between
/// multiple Vms (typically parent and child after fork).  Lazy CoW: any
/// write path that walks into a SHARED PT via `traverse_mut` copies up
/// the PT before returning the leaf pointer, updates this descriptor to
/// point at the fresh PT, and clears this bit.  Invariant (nominally):
/// SHARED ⇒ pt_refcount > 1, though a stale SHARED with refcount == 1
/// is tolerated and self-healed by the sole-owner fast path in
/// `traverse_mut`.
const PTE_SHARED_PT: u64 = 1 << 56;

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
            // Use alloc_pt_page so pt_refcount is initialized to 1.  Leaf PTs
            // allocated here get shared at fork time via share_leaf_pt; the
            // share path's pt_ref_inc only works correctly if the count starts
            // at 1, not 0.
            let new_table = alloc_pt_page().expect("failed to allocate page table");
            unsafe {
                new_table.as_mut_ptr::<u8>().write_bytes(0, PAGE_SIZE);
                *entry = new_table.value() as u64 | DESC_VALID | DESC_TABLE;
                // DSB ensures the table descriptor (and the zeroed table it
                // points to) are visible to the hardware page-table walker
                // before we descend into the next level.  Without this, the
                // walker (or QEMU's emulated walker) may see a stale zero
                // entry and fault instead of following the new descriptor.
                core::arch::asm!("dsb ishst", options(nostack));
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

/// Walk PGD→PUD→PMD to find the leaf PTE table base address.
/// Unlike `traverse`, does NOT index into the final level-0 table.
#[inline(always)]
fn traverse_to_pt(
    pgd: PAddr,
    vaddr: UserVAddr,
    allocate: bool,
) -> Option<*mut PageTableEntry> {
    let mut table = pgd.as_mut_ptr::<PageTableEntry>();
    for level in (1..=3).rev() {
        let index = nth_level_table_index(vaddr, level);
        let entry = unsafe { table.offset(index) };
        let mut table_paddr = entry_paddr(unsafe { *entry });
        if table_paddr.value() == 0 {
            if !allocate {
                return None;
            }
            let new_table = alloc_pt_page().expect("failed to allocate page table");
            unsafe {
                new_table.as_mut_ptr::<u8>().write_bytes(0, PAGE_SIZE);
                *entry = new_table.value() as u64 | DESC_VALID | DESC_TABLE;
                core::arch::asm!("dsb ishst", options(nostack));
            }
            table_paddr = new_table;
        }
        table = table_paddr.as_mut_ptr::<PageTableEntry>();
    }
    Some(table)
}

/// Compute the leaf (level-0) page table index for a virtual address.
#[inline(always)]
fn leaf_pt_index(vaddr_value: usize) -> usize {
    (vaddr_value >> 12) & 0x1FF
}

/// Allocate a new page-table page.  Caller receives DIRTY memory (random
/// bytes); initialize with write_bytes or bulk-copy before use.  Registers
/// the page's paddr in the PT refcount table at count=1.
#[inline]
fn alloc_pt_page() -> Result<PAddr, PageAllocError> {
    let paddr = alloc_pages(1, AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK)?;
    crate::pt_refcount::pt_ref_init(paddr);
    Ok(paddr)
}

/// Broadcast TLB flush after a page-table structural change.  Use when
/// unsharing a PT page: stale walks on other CPUs must stop before the
/// old PT is freed.
#[inline(always)]
fn tlb_flush_all_broadcast() {
    unsafe {
        // dsb ishst: earlier stores globally visible
        // tlbi vmalle1is: invalidate all EL1 TLB entries, inner shareable
        // dsb ish: wait for completion
        // isb: stop speculative execution past the flush
        core::arch::asm!(
            "dsb ishst",
            "tlbi vmalle1is",
            "dsb ish",
            "isb",
            options(nostack),
        );
    }
}

/// Copy-up a shared leaf PT — the write-side half of lazy PT-page CoW.
///
/// Given `pmd_entry_ptr` = pointer to the level-2 (PMD) descriptor whose
/// PTE_SHARED_PT bit is set:
///   - If `pt_refcount > 1`: allocate a fresh PT page, copy the old PT's
///     contents, update the PMD entry to point at the fresh PT (clearing
///     SHARED), broadcast-flush TLBs, and dec old PT refcount (freeing
///     the old PT if we're the last owner).
///   - If `pt_refcount == 1` (sole owner — stale SHARED bit): just clear
///     the SHARED bit in the PMD entry in place.  No copy needed.
///
/// Returns the (possibly new) level-1 table pointer.
#[inline]
fn unshare_leaf_pt(pmd_entry_ptr: *mut PageTableEntry)
    -> Result<*mut PageTableEntry, PageAllocError>
{
    let pmd_entry = unsafe { *pmd_entry_ptr };
    let old_pt_paddr = entry_paddr(pmd_entry);
    let pmd_flags = entry_flags(pmd_entry) & !PTE_SHARED_PT;

    if crate::pt_refcount::pt_ref_count(old_pt_paddr) > 1 {
        // Real unshare: allocate fresh PT, copy contents, publish.
        let fresh = alloc_pt_page()?;
        let old = old_pt_paddr.as_mut_ptr::<PageTableEntry>();
        let new = fresh.as_mut_ptr::<PageTableEntry>();
        unsafe {
            ptr::copy_nonoverlapping(old, new, ENTRIES_PER_TABLE as usize);
            *pmd_entry_ptr = fresh.value() as u64 | pmd_flags;
        }
        tlb_flush_all_broadcast();
        if crate::pt_refcount::pt_ref_dec(old_pt_paddr) {
            crate::page_allocator::free_pages(old_pt_paddr, 1);
        }
        Ok(new)
    } else {
        // Sole owner — just clear the stale SHARED bit.
        unsafe {
            *pmd_entry_ptr = old_pt_paddr.value() as u64 | pmd_flags;
            core::arch::asm!("dsb ishst", options(nostack));
        }
        Ok(old_pt_paddr.as_mut_ptr::<PageTableEntry>())
    }
}

/// Walk PGD → PUD → PMD with intent to mutate a leaf PTE.  Unshares any
/// SHARED leaf PT we encounter before returning the leaf pointer.  Used
/// by `map_user_page*`, `unmap_user_page`, `update_page_flags`,
/// `try_map_user_page_with_prot`, and the CoW fault path.
fn traverse_mut(
    pgd: PAddr,
    vaddr: UserVAddr,
    allocate: bool,
) -> Option<NonNull<PageTableEntry>> {
    debug_assert!(is_aligned(vaddr.value(), PAGE_SIZE));
    let mut table = pgd.as_mut_ptr::<PageTableEntry>();

    for level in (1..=3).rev() {
        let index = nth_level_table_index(vaddr, level);
        let entry_ptr = unsafe { table.offset(index) };
        let entry = unsafe { *entry_ptr };
        let mut table_paddr = entry_paddr(entry);

        if table_paddr.value() == 0 {
            if !allocate {
                return None;
            }
            let new_table = alloc_pt_page().expect("failed to allocate page table");
            unsafe {
                new_table.as_mut_ptr::<u8>().write_bytes(0, PAGE_SIZE);
                *entry_ptr = new_table.value() as u64 | DESC_VALID | DESC_TABLE;
                core::arch::asm!("dsb ishst", options(nostack));
            }
            table_paddr = new_table;
            table = table_paddr.as_mut_ptr::<PageTableEntry>();
            continue;
        }

        // At level == 1, this descriptor points at the level-0 (leaf) PT.
        // If the leaf PT is SHARED, unshare before the caller mutates it.
        if level == 1 && (entry & PTE_SHARED_PT) != 0 {
            table = unshare_leaf_pt(entry_ptr).ok()?;
        } else {
            table = table_paddr.as_mut_ptr::<PageTableEntry>();
        }
    }

    unsafe {
        Some(NonNull::new_unchecked(
            table.offset(nth_level_table_index(vaddr, 0)),
        ))
    }
}

/// Like `traverse_to_pt` but unshares a SHARED leaf PT before returning
/// its base pointer.  Used by `batch_try_map_user_pages_with_prot`.
#[inline(always)]
fn traverse_to_pt_mut(
    pgd: PAddr,
    vaddr: UserVAddr,
    allocate: bool,
) -> Option<*mut PageTableEntry> {
    let mut table = pgd.as_mut_ptr::<PageTableEntry>();
    for level in (1..=3).rev() {
        let index = nth_level_table_index(vaddr, level);
        let entry_ptr = unsafe { table.offset(index) };
        let entry = unsafe { *entry_ptr };
        let mut table_paddr = entry_paddr(entry);

        if table_paddr.value() == 0 {
            if !allocate {
                return None;
            }
            let new_table = alloc_pt_page().expect("failed to allocate page table");
            unsafe {
                new_table.as_mut_ptr::<u8>().write_bytes(0, PAGE_SIZE);
                *entry_ptr = new_table.value() as u64 | DESC_VALID | DESC_TABLE;
                core::arch::asm!("dsb ishst", options(nostack));
            }
            table_paddr = new_table;
            table = table_paddr.as_mut_ptr::<PageTableEntry>();
            continue;
        }

        if level == 1 && (entry & PTE_SHARED_PT) != 0 {
            table = unshare_leaf_pt(entry_ptr).ok()?;
        } else {
            table = table_paddr.as_mut_ptr::<PageTableEntry>();
        }
    }
    Some(table)
}

/// Decrement refcounts on all user pages and free intermediate page table
/// pages, but never free data pages. Safe for forked page tables.
fn teardown_table_dec_only(table_paddr: PAddr, level: usize) {
    if table_paddr.is_null() {
        return;
    }
    let table = table_paddr.as_mut_ptr::<PageTableEntry>();

    // Sparse-table batch skip: OR 8 entries; if all zero, skip the batch.
    // Matches the optimization in `duplicate_table` — typical user page
    // tables have <32 non-null entries of 512, so this eliminates ~94%
    // of iteration work on the exit path too.
    for batch in 0..64isize {
        let base = batch * 8;
        let any = unsafe {
            *table.offset(base)     | *table.offset(base + 1)
            | *table.offset(base + 2) | *table.offset(base + 3)
            | *table.offset(base + 4) | *table.offset(base + 5)
            | *table.offset(base + 6) | *table.offset(base + 7)
        };
        if any == 0 {
            continue;
        }
        for i in base..base + 8 {
            let entry = unsafe { *table.offset(i) };
            let paddr = entry_paddr(entry);
            if paddr.is_null() {
                continue;
            }
            if level == 1 {
                // Leaf PTE: decrement refcount only, never free.
                let rc = crate::page_refcount::page_ref_count(paddr);
                if rc == 0 || rc == crate::page_refcount::PAGE_REF_KERNEL_IMAGE {
                    continue;
                }
                crate::page_refcount::page_ref_dec(paddr);
            } else if level == 2 && (entry & PTE_SHARED_PT) != 0 {
                teardown_leaf_pt_shared(paddr);
                if crate::pt_refcount::pt_ref_dec(paddr) {
                    crate::page_allocator::free_pages(paddr, 1);
                }
            } else {
                // Intermediate table or non-shared leaf: recurse, then
                // free the table page.
                teardown_table_dec_only(paddr, level - 1);
                crate::page_allocator::free_pages(paddr, 1);
            }
        }
    }
}

/// Decrement data-page refcounts in a shared leaf PT.  Called by every
/// owner's teardown (not just the last) so the per-page refcounts stay
/// balanced.  Does NOT touch the PT page itself — caller handles
/// `pt_ref_dec` + free.
#[inline]
fn teardown_leaf_pt_shared(pt_paddr: PAddr) {
    let pt = pt_paddr.as_mut_ptr::<PageTableEntry>();
    for batch in 0..64isize {
        let base = batch * 8;
        let any = unsafe {
            *pt.offset(base)     | *pt.offset(base + 1)
            | *pt.offset(base + 2) | *pt.offset(base + 3)
            | *pt.offset(base + 4) | *pt.offset(base + 5)
            | *pt.offset(base + 6) | *pt.offset(base + 7)
        };
        if any == 0 { continue; }
        for i in base..base + 8 {
            let entry = unsafe { *pt.offset(i) };
            let paddr = entry_paddr(entry);
            if paddr.is_null() { continue; }
            let rc = crate::page_refcount::page_ref_count(paddr);
            if rc == 0 || rc == crate::page_refcount::PAGE_REF_KERNEL_IMAGE {
                continue;
            }
            crate::page_refcount::page_ref_dec(paddr);
        }
    }
}

/// Prepare a leaf PT for sharing between parent and child: iterate in
/// place, bump data-page refcounts, set ATTR_AP_RO on writable entries
/// (so any subsequent write from either owner takes a CoW fault), and
/// publish PTE_SHARED_PT + pt_ref_inc.  Ordering (issue #2 from design
/// review): RO-stamp BEFORE publishing the SHARED bit, with `dsb ish`
/// between, so a concurrent CoW fault on a sibling CPU can't interleave
/// with the stamping loop.
///
/// Called from `duplicate_table` at level == 2 (PMD) for each non-null
/// entry pointing at a level-1 leaf PT.
#[inline]
fn share_leaf_pt(
    pt_paddr: PAddr,
    parent_pmd_entry_ptr: *mut PageTableEntry,
    child_pmd_entry_ptr: *mut PageTableEntry,
) {
    let pt = pt_paddr.as_mut_ptr::<PageTableEntry>();
    // Step 1: RO-stamp pass — bump data-page refcounts, set AP_RO on writable
    // entries in the shared PT.  Sparse-table batch skip for sparse mappings.
    for batch in 0..64isize {
        let base = batch * 8;
        let any = unsafe {
            *pt.offset(base)     | *pt.offset(base + 1)
            | *pt.offset(base + 2) | *pt.offset(base + 3)
            | *pt.offset(base + 4) | *pt.offset(base + 5)
            | *pt.offset(base + 6) | *pt.offset(base + 7)
        };
        if any == 0 {
            continue;
        }
        for i in base..base + 8 {
            let entry = unsafe { *pt.offset(i) };
            let paddr = entry_paddr(entry);
            if paddr.is_null() {
                continue;
            }
            // Bump data-page refcount (new owner = child).
            crate::page_refcount::page_ref_inc(paddr);

            let flags = entry_flags(entry);
            if flags & ATTR_AP_USER != 0 && flags & ATTR_AP_RO == 0 {
                // Set AP_RO so writes from either owner fault into the CoW
                // handler.  Writes go through the shared PT until unshared.
                unsafe {
                    *pt.offset(i) = paddr.value() as u64 | (flags | ATTR_AP_RO);
                }
            }
        }
    }

    // Step 2: barrier — make all stamping stores visible globally before we
    // publish the SHARED marker.  A CoW fault on a remote CPU that reads
    // SHARED then walks into the PT must see the RO-stamped entries.
    unsafe { core::arch::asm!("dsb ish", options(nostack)); }

    // Step 3: bump PT-page refcount — child now shares ownership.
    crate::pt_refcount::pt_ref_inc(pt_paddr);

    // Step 4: publish SHARED bit on both PMD descriptors (in place for
    // parent, OR-in for child — child's PMD already points at the same
    // pt_paddr from the bulk copy of the PMD table above).
    unsafe {
        let parent_entry = *parent_pmd_entry_ptr;
        *parent_pmd_entry_ptr = parent_entry | PTE_SHARED_PT;
        let child_entry = *child_pmd_entry_ptr;
        *child_pmd_entry_ptr = child_entry | PTE_SHARED_PT;
    }
    // Parent's TLB may have cached writable entries for this PT's range
    // from before share_leaf_pt stamped AP_RO in place.  Without a flush,
    // parent will continue writing (via cached TLB) through the shared
    // page, corrupting the shared data visible to the child.  Flush all
    // TLB entries in the inner-shareable domain.
    tlb_flush_all_broadcast();
}

fn duplicate_table(original_table_paddr: PAddr, level: usize) -> Result<PAddr, PageAllocError> {
    let orig_table = original_table_paddr.as_mut_ptr::<PageTableEntry>();
    // DIRTY_OK: we immediately bulk-copy over the page, so pre-zeroing would
    // be pure waste.  Saves ~8 µs/fork on a 4-level tree (zero 4 KB ×
    // 8 PT pages at cached-DRAM speed).  Safe because every byte we
    // don't copy explicitly stays whatever the allocator handed us,
    // and ptr::copy_nonoverlapping covers the entire 4 KB PT.
    let new_table_paddr = alloc_pt_page()?;
    let new_table = new_table_paddr.as_mut_ptr::<PageTableEntry>();

    // Bulk-copy the entire 4 KB page table in one shot.
    unsafe {
        ptr::copy_nonoverlapping(orig_table, new_table, ENTRIES_PER_TABLE as usize);
    }

    debug_assert!(level > 0);

    if level == 1 {
        // Leaf page table (PTE level): only reachable via the non-sharing
        // ghost-fork path.  Preserved for completeness; regular fork uses
        // leaf sharing via the level == 2 branch below and never recurses
        // here.
        for batch in 0..64isize {
            let base = batch * 8;
            let any = unsafe {
                *orig_table.offset(base)     | *orig_table.offset(base + 1)
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
                crate::page_refcount::page_ref_inc(paddr);
                let flags = entry_flags(entry);
                if flags & ATTR_AP_USER != 0 && flags & ATTR_AP_RO == 0 {
                    let cow_entry = paddr.value() as u64 | (flags | ATTR_AP_RO);
                    unsafe {
                        *orig_table.offset(i) = cow_entry;
                        *new_table.offset(i) = cow_entry;
                    }
                }
            }
        }
    } else if level == 2 {
        // PMD level: entries point at leaf PT pages.  Instead of recursing
        // into level 1 (which would alloc a fresh leaf PT per entry and
        // bulk-copy it), *share* each leaf PT with the child.  This is the
        // lazy-CoW win: skip ~1.2 µs per leaf PT alloc+memcpy.
        //
        // The bulk copy above already placed the same leaf-PT paddrs in
        // child's PMD entries.  `share_leaf_pt` handles the rest in place.
        for i in 0..ENTRIES_PER_TABLE {
            let entry = unsafe { *orig_table.offset(i) };
            let pt_paddr = entry_paddr(entry);
            if pt_paddr.is_null() {
                continue;
            }
            // If the entry is already SHARED from a prior fork, we're
            // adding a third (or Nth) owner: just pt_ref_inc, no need to
            // RO-stamp again (the PT is already RO'd) and no need to
            // re-store the SHARED bit (already set).
            if entry & PTE_SHARED_PT != 0 {
                // Still need to bump data-page refcounts for the new owner.
                let pt = pt_paddr.as_mut_ptr::<PageTableEntry>();
                for batch in 0..64isize {
                    let base = batch * 8;
                    let any = unsafe {
                        *pt.offset(base)     | *pt.offset(base + 1)
                        | *pt.offset(base + 2) | *pt.offset(base + 3)
                        | *pt.offset(base + 4) | *pt.offset(base + 5)
                        | *pt.offset(base + 6) | *pt.offset(base + 7)
                    };
                    if any == 0 { continue; }
                    for j in base..base + 8 {
                        let e = unsafe { *pt.offset(j) };
                        let p = entry_paddr(e);
                        if !p.is_null() {
                            crate::page_refcount::page_ref_inc(p);
                        }
                    }
                }
                crate::pt_refcount::pt_ref_inc(pt_paddr);
                // child's PMD entry carries SHARED (copied from parent).
            } else {
                let parent_pmd = unsafe { orig_table.offset(i) };
                let child_pmd = unsafe { new_table.offset(i) };
                share_leaf_pt(pt_paddr, parent_pmd, child_pmd);
            }
        }
    } else {
        // Intermediate table (PGD/PUD): recurse to duplicate sub-tables.
        // The bulk copy left child pointers pointing at the parent's
        // sub-tables — we must rewrite each with a fresh duplicated one.
        for i in 0..ENTRIES_PER_TABLE {
            let entry = unsafe { *orig_table.offset(i) };
            let paddr = entry_paddr(entry);
            if paddr.is_null() {
                continue;
            }
            let sub_table = duplicate_table(paddr, level - 1)?;
            unsafe {
                *new_table.offset(i) = sub_table.value() as u64 | entry_flags(entry);
            }
        }
    }

    Ok(new_table_paddr)
}

/// Ghost-fork duplicate: same PT structure + CoW marking as `duplicate_table`,
/// but skips every refcount mutation.  The parent is blocked until the child
/// exec's or _exits, so no concurrent reader needs the refcount bump for
/// safety; skipping it saves ~40 ns per writable PTE (typical ~200 pages =
/// ~8 µs).  Collects the CoW-marked virtual addresses so the eventual
/// `restore_writable_from_list` scan is O(cow_pages) not O(all_PTEs).
///
/// Mirrors `platform/x64/paging.rs::duplicate_table_ghost`.  ARM64 has no
/// huge-page code path (see `map_huge_user_page` — it splits to 4 KB),
/// so we omit the level-2 block descriptor case that exists on x86_64.
fn duplicate_table_ghost(
    original_table_paddr: PAddr,
    level: usize,
    base_vaddr: usize,
    cow_addrs: &mut Vec<usize>,
) -> Result<PAddr, PageAllocError> {
    let orig_table = original_table_paddr.as_mut_ptr::<PageTableEntry>();
    // Same DIRTY_OK rationale as duplicate_table — we're about to bulk-copy.
    let new_table_paddr = alloc_pt_page()?;
    let new_table = new_table_paddr.as_mut_ptr::<PageTableEntry>();

    unsafe {
        ptr::copy_nonoverlapping(orig_table, new_table, ENTRIES_PER_TABLE as usize);
    }

    debug_assert!(level > 0);

    if level == 1 {
        // Leaf PTEs: CoW-mark writable user pages without touching refcount.
        // Leave non-writable and kernel pages alone — they're already shared
        // correctly by the bulk copy.
        for i in 0..ENTRIES_PER_TABLE {
            let entry = unsafe { *orig_table.offset(i) };
            let paddr = entry_paddr(entry);
            if paddr.is_null() {
                continue;
            }
            let flags = entry_flags(entry);
            if flags & ATTR_AP_USER == 0 {
                continue;
            }
            if flags & ATTR_AP_RO == 0 {
                // Parent+child PTE: set ATTR_AP_RO, remember original writability
                // via PTE_WAS_WRITABLE so restore_writable_from_list can reverse it.
                let cow_entry = paddr.value() as u64 | flags | ATTR_AP_RO | PTE_WAS_WRITABLE;
                unsafe {
                    *orig_table.offset(i) = cow_entry;
                    *new_table.offset(i) = cow_entry;
                }
                cow_addrs.push(base_vaddr | ((i as usize) << 12));
            }
        }
    } else {
        for i in 0..ENTRIES_PER_TABLE {
            let entry = unsafe { *orig_table.offset(i) };
            let paddr = entry_paddr(entry);
            if paddr.is_null() {
                continue;
            }
            // If the parent's PMD entry points at a SHARED leaf PT (from
            // a prior regular fork), unshare the parent side FIRST so
            // the ghost fork's in-place AP_RO stamping doesn't leak
            // into the other owner's view of the PT.  After unshare,
            // parent has an exclusive fresh leaf PT, and the ghost
            // fork recursion proceeds on that.  (Design review issue
            // #12: ghost-fork coexistence with lazy-CoW sharing.)
            let (effective_paddr, effective_entry_flags) =
                if level == 2 && (entry & PTE_SHARED_PT) != 0 {
                    let parent_pmd = unsafe { orig_table.offset(i) };
                    // Unshare the parent side; now parent_pmd points at a
                    // fresh (exclusive) PT.  Read the new paddr from the
                    // updated PMD entry.
                    let _ = unshare_leaf_pt(parent_pmd)
                        .map_err(|_| PageAllocError)?;
                    let updated_parent_entry = unsafe { *parent_pmd };
                    let unshared_paddr = entry_paddr(updated_parent_entry);
                    // Clear SHARED on child's bulk-copied PMD entry so the
                    // recursion below proceeds as a normal duplicate.
                    unsafe {
                        let child_entry = *new_table.offset(i);
                        *new_table.offset(i) = child_entry & !PTE_SHARED_PT;
                    }
                    (unshared_paddr, entry_flags(updated_parent_entry))
                } else {
                    (paddr, entry_flags(entry))
                };
            let shift = (level - 1) * 9 + 12;
            let child_base = base_vaddr | ((i as usize) << shift);
            let new_child_paddr = duplicate_table_ghost(
                effective_paddr, level - 1, child_base, cow_addrs)?;
            unsafe {
                *new_table.offset(i) = new_child_paddr.value() as u64
                    | effective_entry_flags;
            }
        }
    }

    Ok(new_table_paddr)
}

/// Free page-table pages from a ghost-forked page table.  Data pages owned
/// by the parent are left alone (they were never refcount-bumped during the
/// ghost fork).  Child-owned CoW-copies — PTEs that lost both ATTR_AP_RO and
/// PTE_WAS_WRITABLE during a CoW fault — are decremented and freed.
fn teardown_table_ghost(table_paddr: PAddr, level: usize) {
    if table_paddr.is_null() {
        return;
    }
    let table = table_paddr.as_mut_ptr::<PageTableEntry>();

    // Same sparse-table batch skip used in `teardown_table_dec_only`.
    for batch in 0..64isize {
        let base = batch * 8;
        let any = unsafe {
            *table.offset(base)     | *table.offset(base + 1)
            | *table.offset(base + 2) | *table.offset(base + 3)
            | *table.offset(base + 4) | *table.offset(base + 5)
            | *table.offset(base + 6) | *table.offset(base + 7)
        };
        if any == 0 {
            continue;
        }
        for i in base..base + 8 {
            let entry = unsafe { *table.offset(i) };
            let paddr = entry_paddr(entry);
            if paddr.is_null() {
                continue;
            }

            if level == 1 {
                let flags = entry_flags(entry);
                if flags & ATTR_AP_USER == 0 {
                    continue;
                }
                // Child-owned if the CoW fault rewrote this PTE: writable and
                // no PTE_WAS_WRITABLE (CoW writes install a fresh writable
                // PTE pointing at a newly-allocated paddr).
                if flags & ATTR_AP_RO == 0 && entry & PTE_WAS_WRITABLE == 0 {
                    let rc = crate::page_refcount::page_ref_count(paddr);
                    if rc == 1 {
                        crate::page_refcount::page_ref_dec(paddr);
                        crate::page_allocator::free_pages(paddr, 1);
                    }
                }
                // Else: parent-owned (read-only shared or still CoW-marked);
                // leave untouched.
            } else if level == 2 && (entry & PTE_SHARED_PT) != 0 {
                // Shared leaf PT (regular fork semantics, not ghost): same
                // teardown rules as `teardown_table_dec_only`.  Ghost fork
                // shouldn't ordinarily produce SHARED entries because
                // `duplicate_table_ghost` unshares them first, but handle
                // defensively in case the interaction is reached.
                teardown_leaf_pt_shared(paddr);
                if crate::pt_refcount::pt_ref_dec(paddr) {
                    crate::page_allocator::free_pages(paddr, 1);
                }
            } else {
                teardown_table_ghost(paddr, level - 1);
                crate::page_allocator::free_pages(paddr, 1);
            }
        }
    }
}

/// Targeted restore of writability: clear ATTR_AP_RO | PTE_WAS_WRITABLE on
/// every PTE the ghost fork marked, using the collected address list.
/// O(cow_pages) rather than O(all_PTEs).
fn restore_writable_from_list(pgd: PAddr, addrs: &[usize]) {
    for &vaddr in addrs {
        let uva = unsafe { UserVAddr::new_unchecked(vaddr) };
        // `traverse_mut` (not `traverse`) so we unshare any SHARED leaf PT
        // before clearing AP_RO — otherwise we'd silently make the other
        // owner's PTE writable too (design review issue #1).
        if let Some(mut pte) = traverse_mut(pgd, uva, false) {
            let entry = unsafe { *pte.as_ptr() };
            if entry & PTE_WAS_WRITABLE != 0 {
                let restored = (entry & !ATTR_AP_RO) & !PTE_WAS_WRITABLE;
                unsafe {
                    *pte.as_mut() = restored;
                    core::arch::asm!("dsb ish", "isb", options(nostack));
                }
            }
        }
    }
}

fn allocate_pgd() -> Result<PAddr, PageAllocError> {
    // Allocate a fresh user PGD (TTBR0). No kernel entries needed since
    // kernel uses TTBR1 exclusively.  Goes through `alloc_pt_page` so the
    // PGD gets pt_refcount=1 for symmetry — the PGD is never shared but
    // teardown_forked_pages + teardown_user_pages call `free_pages` on it
    // without going through `pt_ref_dec`, which is fine: unused pt_ref
    // slots are benign.
    let pgd = alloc_pt_page()?;
    unsafe {
        pgd.as_mut_ptr::<u8>().write_bytes(0, PAGE_SIZE);
    }
    Ok(pgd)
}

/// Translate Linux mmap prot flags to ARM64 PTE attributes for a user page.
fn prot_to_attrs(prot_flags: i32) -> u64 {
    let mut attrs = DESC_VALID | DESC_PAGE | ATTR_IDX_NORMAL | ATTR_SH_ISH | ATTR_AF | ATTR_NG;

    if prot_flags & 3 != 0 {
        // PROT_READ or PROT_WRITE → user-space accessible (AP[1]=1).
        // PROT_NONE leaves AP[1]=0 so EL0 access causes a permission fault.
        attrs |= ATTR_AP_USER;
    }
    if prot_flags & 2 == 0 {
        // No PROT_WRITE → read-only (AP[2]=1).
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

    pub fn duplicate_from(original: &mut PageTable) -> Result<PageTable, PageAllocError> {
        Ok(PageTable {
            pgd: duplicate_table(original.pgd, 4)?,
        })
    }

    /// Decrement refcounts and free intermediate page table pages, but never
    /// free data pages. Safe for forked page tables.
    pub fn teardown_forked_pages(&mut self) {
        teardown_table_dec_only(self.pgd, 4);
        crate::page_allocator::free_pages(self.pgd, 1);
        self.pgd = PAddr::new(0);
    }

    pub fn switch(&self) {
        // `dsb ish` is the lighter barrier — inner-shareable domain is enough
        // for the local `tlbi vmalle1` below (not an _is_ broadcast).  `dsb sy`
        // costs extra on Apple Silicon HVF because it forces full-system
        // ordering that the hypervisor has to arbitrate.  Linux arm64's
        // switch_mm uses `dsb ish` for the same reason.
        unsafe {
            core::arch::asm!(
                "msr ttbr0_el1, {}",
                "tlbi vmalle1",
                "dsb ish",
                "isb",
                in(reg) self.pgd.value() as u64,
            );
        }
    }

    pub fn map_user_page(&mut self, vaddr: UserVAddr, paddr: PAddr) {
        // Default: RW, no exec.
        let attrs = DESC_VALID | DESC_PAGE | ATTR_IDX_NORMAL | ATTR_SH_ISH
            | ATTR_AF | ATTR_NG | ATTR_AP_USER | ATTR_PXN | ATTR_UXN;
        crate::page_refcount::page_ref_init(paddr);
        self.map_page(vaddr, paddr, attrs);
    }

    pub fn map_user_page_with_prot(&mut self, vaddr: UserVAddr, paddr: PAddr, prot_flags: i32) {
        let attrs = prot_to_attrs(prot_flags);
        crate::page_refcount::page_ref_init(paddr);
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
        let entry_ptr = match traverse_mut(self.pgd, vaddr, true) {
            Some(ptr) => ptr,
            None => return false,
        };
        let ok = unsafe {
            let atom = &*(entry_ptr.as_ptr() as *const core::sync::atomic::AtomicU64);
            let new_val = paddr.value() as u64 | attrs;
            let ok = atom.compare_exchange(0, new_val,
                core::sync::atomic::Ordering::AcqRel,
                core::sync::atomic::Ordering::Relaxed).is_ok();
            if ok {
                core::arch::asm!("dsb ish", "isb", options(nostack));
            }
            ok
        };
        if ok {
            crate::flight_recorder::record(
                crate::flight_recorder::kind::MAP_USER,
                0, vaddr.value() as u64, paddr.value() as u64,
            );
        }
        ok
    }

    /// Batch-map contiguous user pages, traversing the page table hierarchy only
    /// once per leaf PT (2MB region) instead of once per page.
    /// Returns a u32 bitmask: bit i set = page i was mapped.
    #[inline(always)]
    pub fn batch_try_map_user_pages_with_prot(
        &mut self,
        start_vaddr: UserVAddr,
        paddrs: &[PAddr],
        count: usize,
        prot_flags: i32,
    ) -> u32 {
        let attrs = prot_to_attrs(prot_flags);
        let mut mapped: u32 = 0;
        let mut i = 0;

        while i < count {
            let vaddr_value = start_vaddr.value() + i * PAGE_SIZE;
            let pt_base = match traverse_to_pt_mut(
                self.pgd,
                UserVAddr::new_nonnull(vaddr_value).unwrap(),
                true,
            ) {
                Some(ptr) => ptr,
                None => break,
            };

            let start_idx = leaf_pt_index(vaddr_value);
            let remaining_in_pt = ENTRIES_PER_TABLE as usize - start_idx;
            let batch_end = if i + remaining_in_pt < count { i + remaining_in_pt } else { count };

            let mut idx = start_idx;
            while i < batch_end {
                let entry_ptr = unsafe { pt_base.add(idx) };
                let new_val = paddrs[i].value() as u64 | attrs;
                let atom = unsafe { &*(entry_ptr as *const core::sync::atomic::AtomicU64) };
                if atom.compare_exchange(0, new_val,
                    core::sync::atomic::Ordering::AcqRel,
                    core::sync::atomic::Ordering::Relaxed).is_ok()
                {
                    mapped |= 1 << i;
                    let vaddr_val = start_vaddr.value() + i * PAGE_SIZE;
                    crate::flight_recorder::record(
                        crate::flight_recorder::kind::MAP_USER,
                        0, vaddr_val as u64, paddrs[i].value() as u64,
                    );
                }
                idx += 1;
                i += 1;
            }
        }

        if mapped != 0 {
            unsafe { core::arch::asm!("dsb ish", "isb", options(nostack)); }
        }
        mapped
    }

    pub fn update_page_flags(&mut self, vaddr: UserVAddr, prot_flags: i32) -> bool {
        let entry_ptr = match traverse_mut(self.pgd, vaddr, false) {
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
            core::arch::asm!("dsb ish", "isb", options(nostack));
        }
        true
    }

    pub fn unmap_user_page(&mut self, vaddr: UserVAddr) -> Option<PAddr> {
        let entry_ptr = match traverse_mut(self.pgd, vaddr, false) {
            Some(ptr) => ptr,
            None => return None,
        };

        let entry = unsafe { *entry_ptr.as_ptr() };
        let paddr = entry_paddr(entry);
        if paddr.is_null() {
            return None;
        }

        // Clear the PTE.  The caller is responsible for refcount management
        // and freeing the page — mremap, for instance, remaps the page at a
        // new address without freeing it.
        unsafe {
            *entry_ptr.as_ptr() = 0;
        }

        crate::flight_recorder::record(
            crate::flight_recorder::kind::UNMAP_USER,
            0, vaddr.value() as u64, paddr.value() as u64,
        );

        Some(paddr)
    }

    pub fn flush_tlb(&self, vaddr: UserVAddr) {
        // `tlbi vale1, {op}` operand: VA[55:12] in bits [43:0], ASID in
        // bits [63:48].  With TCR.AS=0 and TTBR0 always written with
        // ASID=0, this operand's ASID field is always 0 — correct.
        // When we later enable ASID tagging (blog 216 / 217), this
        // site needs to thread the live ASID through, or ASID-tagged
        // entries will stay stale after the kernel fixes the PTE.
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
        // `dsb ish` — inner-shareable barrier.  Matches Linux arm64 (see
        // arch/arm64/include/asm/tlbflush.h `__flush_tlb_all`).  `dsb sy`
        // costs extra on HVF because it serializes against host-level
        // memory ordering.
        unsafe {
            core::arch::asm!(
                "tlbi vmalle1",
                "dsb ish",
                "isb",
            );
        }
    }

    fn map_page(&mut self, vaddr: UserVAddr, paddr: PAddr, attrs: u64) {
        debug_assert!(is_aligned(vaddr.value(), PAGE_SIZE));
        let mut entry = traverse_mut(self.pgd, vaddr, true).unwrap();
        unsafe {
            *entry.as_mut() = paddr.value() as u64 | attrs;
            // ARM ARM B2.9: DSB ensures the PTE store is visible to the page
            // table walker before any subsequent access through the new mapping.
            // ISB ensures the CPU fetches new translations.
            core::arch::asm!("dsb ish", "isb", options(nostack));
        }
        crate::flight_recorder::record(
            crate::flight_recorder::kind::MAP_USER,
            0, vaddr.value() as u64, paddr.value() as u64,
        );
    }

    // ── Lookup helpers ────────────────────────────────────────────────────────

    /// Look up the physical address for a mapped user page.
    pub fn lookup_paddr(&self, vaddr: UserVAddr) -> Option<PAddr> {
        let entry_ptr = traverse(self.pgd, vaddr, false)?;
        let entry = unsafe { *entry_ptr.as_ptr() };
        let paddr = entry_paddr(entry);
        if paddr.is_null() { None } else { Some(paddr) }
    }

    /// Look up the raw PTE value for a mapped user page (flags + physical address).
    pub fn lookup_pte_entry(&self, vaddr: UserVAddr) -> Option<u64> {
        let entry_ptr = traverse(self.pgd, vaddr, false)?;
        let entry = unsafe { *entry_ptr.as_ptr() };
        if entry != 0 { Some(entry) } else { None }
    }

    // ── Huge page stubs (ARM64 uses 4KB pages only; no 2MB TLB optimization) ──

    /// Stub: map 512 individual 4KB pages covering the 2MB region at `vaddr`.
    pub fn map_huge_user_page(&mut self, vaddr: UserVAddr, paddr: PAddr, prot_flags: i32) {
        for i in 0..512 {
            let pv = UserVAddr::new_nonnull(vaddr.value() + i * PAGE_SIZE).unwrap();
            let pp = PAddr::new(paddr.value() + i * PAGE_SIZE);
            self.map_user_page_with_prot(pv, pp, prot_flags);
        }
    }

    /// Stub: unmap 512 individual 4KB pages; returns the base physical address.
    pub fn unmap_huge_user_page(&mut self, vaddr: UserVAddr) -> Option<PAddr> {
        let base = self.unmap_user_page(vaddr);
        for i in 1..512 {
            let pv = UserVAddr::new_nonnull(vaddr.value() + i * PAGE_SIZE).unwrap();
            self.unmap_user_page(pv);
        }
        base
    }

    /// Stub: ARM64 never uses 2MB huge-page descriptors — always returns `None`.
    #[inline(always)]
    pub fn is_huge_mapped(&self, _vaddr: UserVAddr) -> Option<u64> {
        None
    }

    /// Stub: returns `true` if the first 4KB PTE in the 2MB region is absent.
    pub fn is_pde_empty(&self, vaddr: UserVAddr) -> bool {
        match traverse(self.pgd, vaddr, false) {
            Some(ptr) => unsafe { *ptr.as_ptr() == 0 },
            None => true,
        }
    }

    /// Stub: no huge pages to split — returns `None`.
    #[inline(always)]
    pub fn split_huge_page(&mut self, _vaddr: UserVAddr) -> Option<PAddr> {
        None
    }

    /// Stub: no huge-page PDE to update — returns `false`.
    #[inline(always)]
    pub fn update_huge_page_flags(&mut self, _vaddr: UserVAddr, _prot_flags: i32) -> bool {
        false
    }

    /// Returns the physical address of the top-level page table (PGD).
    /// Named `pml4` for cross-arch API compatibility with x86_64.
    pub fn pml4(&self) -> PAddr {
        self.pgd
    }

    /// Zero out the root-table field so the original's drop path won't
    /// double-free after we've handed it off to the deferred teardown list.
    pub fn clear_pml4_for_defer(&mut self) {
        self.pgd = PAddr::new(0);
    }

    /// Construct a PageTable from an existing root for the sole purpose of
    /// running `teardown_forked_pages` on it. Mirror of x86_64's
    /// `from_pml4_for_teardown` — the name uses x86 terminology for API
    /// parity with the deferred-Vm-drop path in kernel/mm/vm.rs.
    pub fn from_pml4_for_teardown(pgd: PAddr) -> PageTable {
        PageTable { pgd }
    }

    /// Ghost-fork: duplicate page-table structure, CoW-mark writable leaves
    /// without touching refcounts, collect the CoW-marked virtual addresses
    /// so `restore_writable_from` is O(cow_pages) on the ghost-child exit.
    /// Safe only when the parent is blocked (enforced by sys_fork's
    /// GHOST_FORK_ENABLED path and sys_clone's vfork path).
    pub fn duplicate_from_ghost(original: &PageTable)
        -> Result<(PageTable, Vec<usize>), PageAllocError>
    {
        let mut cow_addrs = Vec::new();
        let new_pgd = duplicate_table_ghost(original.pgd, 4, 0, &mut cow_addrs)?;
        // Make the CoW demotions visible to the parent's TLB before the
        // child starts running; the flush happens via `flush_tlb_all()`
        // in the vfork / ghost fork exit path, but an earlier ISB keeps
        // the parent from executing a stale writable translation on
        // SMP boundaries.
        unsafe { core::arch::asm!("dsb ish", "isb", options(nostack)); }
        Ok((PageTable { pgd: new_pgd }, cow_addrs))
    }

    /// Teardown for a ghost-forked page table: free page-table pages and
    /// any child-allocated CoW copies, but leave parent-owned data pages
    /// alone (they were never refcount-bumped during the ghost fork).
    pub fn teardown_ghost_pages(&mut self) {
        teardown_table_ghost(self.pgd, 4);
        let pgd = self.pgd;
        self.pgd = PAddr::new(0);
        crate::page_allocator::free_pages(pgd, 1);
    }

    /// Restore ATTR_AP_RO → writable on the parent's PTEs listed in `addrs`,
    /// clearing PTE_WAS_WRITABLE at the same time.  Called after the ghost
    /// child exec's or _exits.
    pub fn restore_writable_from(&mut self, addrs: &[usize]) {
        restore_writable_from_list(self.pgd, addrs);
    }

    /// Map a device memory page (MMIO). Uses MAIR attr0 = Device-nGnRnE so
    /// writes bypass the cache and go directly to hardware — required for
    /// framebuffers and similar BAR-backed regions.
    #[inline(always)]
    pub fn map_device_page(&mut self, vaddr: UserVAddr, paddr: PAddr, prot_flags: i32) {
        // Start from device attributes (not ATTR_IDX_NORMAL).
        let mut attrs = DESC_VALID | DESC_PAGE | ATTR_IDX_DEVICE | ATTR_SH_ISH
            | ATTR_AF | ATTR_NG | ATTR_AP_USER | ATTR_PXN;
        // Writable bit is cleared by setting AP[2] (read-only); clear
        // it when PROT_WRITE is requested.
        if prot_flags & 2 == 0 {
            attrs |= ATTR_AP_RO;
        }
        // Always UXN (no exec) for device memory.
        attrs |= ATTR_UXN;
        // Device pages are not page-allocator-managed — no refcount.
        self.map_page(vaddr, paddr, attrs);
    }
}
