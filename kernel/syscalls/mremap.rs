// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! mremap(2) syscall handler.
//!
//! Supports MREMAP_MAYMOVE on anonymous mappings.
//!
//! Provenance: Own (Linux mremap(2) man page).
use crate::mm::vm::VmAreaType;
use crate::prelude::*;
use crate::process::current_process;
use crate::syscalls::SyscallHandler;
use kevlar_platform::{
    address::{PAddr, UserVAddr},
    arch::{PAGE_SIZE, HUGE_PAGE_SIZE},
    page_allocator::free_pages,
};
use kevlar_utils::alignment::{align_up, is_aligned};

const MREMAP_MAYMOVE: i32 = 1;
const MREMAP_FIXED: i32 = 2;

impl<'a> SyscallHandler<'a> {
    pub fn sys_mremap(
        &mut self,
        old_addr: UserVAddr,
        old_size: usize,
        new_size: usize,
        flags: i32,
    ) -> Result<isize> {
        // Validate alignment.
        if !is_aligned(old_addr.value(), PAGE_SIZE) {
            return Err(Errno::EINVAL.into());
        }
        if old_size == 0 || new_size == 0 {
            return Err(Errno::EINVAL.into());
        }

        // MREMAP_FIXED not supported yet.
        if flags & MREMAP_FIXED != 0 {
            return Err(Errno::EINVAL.into());
        }
        // Only 0 or MREMAP_MAYMOVE are valid flag values (for now).
        if flags & !(MREMAP_MAYMOVE) != 0 {
            return Err(Errno::EINVAL.into());
        }

        let old_size = align_up(old_size, PAGE_SIZE);
        let new_size = align_up(new_size, PAGE_SIZE);

        let may_move = flags & MREMAP_MAYMOVE != 0;

        let current = current_process();
        let vm_ref = current.vm();
        let mut vm = vm_ref.as_ref().unwrap().lock_preempt();

        // ktrace: sample page content at mremap entry.
        // Phase 3 Method B diagnostic: read user VA via copy_from_user AND
        // read the physical page directly.  If user_byte == 0xAB but pa_byte
        // == 0x00, the user write happened but the kernel sees stale PA
        // (cache coherency bug).  If both are 0x00, the user write never
        // executed (QEMU TCG instruction replay bug).
        #[cfg(feature = "ktrace-mm")]
        if let Some(pa) = vm.page_table().lookup_paddr(old_addr) {
            let b = kevlar_platform::page_ops::page_as_slice(pa);
            let pa_word = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            // Read through user VA (goes through the MMU/TLB path).
            let mut user_buf = [0u8; 4];
            let _ = old_addr.read_bytes(&mut user_buf);
            let user_word = u32::from_le_bytes(user_buf);
            crate::debug::ktrace::trace(
                crate::debug::ktrace::event::PAGE_CONTENT,
                pa.value() as u32, (pa.value() >> 32) as u32,
                pa_word, 1 /* context: 1=mremap_entry */, user_word,
            );
            // If the two differ, it's a cache coherency issue.
            // If both are zero, the user memset never executed.
            if pa_word != user_word {
                warn!(
                    "mremap diag: PA byte={:#x} vs user VA byte={:#x} (coherency mismatch!)",
                    pa_word, user_word
                );
            }
        }

        // Find the VMA containing old_addr.
        let (_vma_idx, _vma_start, vma_len, vma_prot, vma_shared) = {
            let mut found = None;
            for (i, vma) in vm.vm_areas().iter().enumerate() {
                if vma.start() == old_addr && vma.end().value() >= old_addr.value() + old_size {
                    // Only support anonymous mappings for now.
                    if !matches!(vma.area_type(), VmAreaType::Anonymous) {
                        return Err(Errno::EINVAL.into());
                    }
                    found = Some((i, vma.start(), vma.end().value() - vma.start().value(), vma.prot(), vma.is_shared()));
                    break;
                }
            }
            match found {
                Some(f) => f,
                None => return Err(Errno::EFAULT.into()),
            }
        };

        // Same size: no-op.
        if new_size == old_size {
            return Ok(old_addr.value() as isize);
        }

        // Shrink: unmap excess pages and trim VMA.
        if new_size < old_size {
            let trim_start = UserVAddr::new_nonnull(old_addr.value() + new_size)?;
            let trim_len = old_size - new_size;

            // Remove the excess VMA range.
            vm.remove_vma_range(trim_start, trim_len)?;

            // Unmap physical pages in the trimmed region.
            let mut to_free: alloc::vec::Vec<(PAddr, usize)> = alloc::vec::Vec::new();
            let end_value = trim_start.value() + trim_len;
            let mut cursor = trim_start.value();
            while cursor < end_value {
                let page_addr = UserVAddr::new(cursor).unwrap();

                // Handle huge pages.
                if is_aligned(cursor, HUGE_PAGE_SIZE) && cursor + HUGE_PAGE_SIZE <= end_value {
                    if let Some(hp_paddr) = vm.page_table_mut().unmap_huge_user_page(page_addr) {
                        vm.page_table().flush_tlb_local(page_addr);
                        for sub_i in 0..512usize {
                            to_free.push((PAddr::new(hp_paddr.value() + sub_i * PAGE_SIZE), 1));
                        }
                        cursor += HUGE_PAGE_SIZE;
                        continue;
                    }
                }

                if vm.page_table().is_huge_mapped(page_addr).is_some() {
                    vm.page_table_mut().split_huge_page(page_addr);
                    vm.page_table().flush_tlb_local(page_addr);
                }

                if let Some(paddr) = vm.page_table_mut().unmap_user_page(page_addr) {
                    vm.page_table().flush_tlb_local(page_addr);
                    to_free.push((paddr, 1));
                }
                cursor += PAGE_SIZE;
            }

            if !to_free.is_empty() {
                vm.page_table().flush_tlb_remote();
            }

            for (paddr, num) in to_free {
                if kevlar_platform::page_refcount::page_ref_dec(paddr) {
                    free_pages(paddr, num);
                }
            }

            return Ok(old_addr.value() as isize);
        }

        // Grow: try to extend in-place first.
        let _extend_start = UserVAddr::new_nonnull(old_addr.value() + vma_len)?;

        // If the VMA is already larger than old_size (i.e., old_size was a
        // subset), we only need to check from the VMA's actual end.
        if vma_len >= new_size {
            // VMA already covers new_size — no-op.
            return Ok(old_addr.value() as isize);
        }

        // Check if space after the current VMA end is free.
        let vma_end_addr = UserVAddr::new_nonnull(old_addr.value() + vma_len)?;
        let needed = new_size - vma_len;
        if vm.is_free_vaddr_range(vma_end_addr, needed) {
            // Extend in-place: grow the existing VMA.
            // Debug: check page content before extending.
            // In-place extension — no page movement needed.
            vm.extend_vma(old_addr, needed)?;
            return Ok(old_addr.value() as isize);
        }

        // Can't extend in-place. If MREMAP_MAYMOVE, relocate.
        if !may_move {
            return Err(Errno::ENOMEM.into());
        }

        // Allocate new virtual address range.
        let new_addr = if new_size >= HUGE_PAGE_SIZE {
            vm.alloc_vaddr_range_aligned(new_size, HUGE_PAGE_SIZE)?
        } else {
            vm.alloc_vaddr_range(new_size)?
        };

        // Add VMA for the new region.
        vm.add_vm_area_with_prot(
            new_addr,
            new_size,
            VmAreaType::Anonymous,
            vma_prot,
            vma_shared,
        )?;

        // Move existing page mappings from old to new address.
        // Pages beyond old_size will be demand-faulted (zeroed).
        let old_pages = old_size / PAGE_SIZE;
        for i in 0..old_pages {
            let old_page = UserVAddr::new_nonnull(old_addr.value() + i * PAGE_SIZE)?;
            let new_page = UserVAddr::new_nonnull(new_addr.value() + i * PAGE_SIZE)?;

            // Handle huge pages: split them first.
            if vm.page_table().is_huge_mapped(old_page).is_some() {
                vm.page_table_mut().split_huge_page(old_page);
                vm.page_table().flush_tlb_local(old_page);
            }

            if let Some(paddr) = vm.page_table_mut().unmap_user_page(old_page) {
                vm.page_table().flush_tlb_local(old_page);
                let prot_flags = vma_prot.bits();
                vm.page_table_mut().map_user_page_with_prot(new_page, paddr, prot_flags);
                // No refcount change — same physical page, just moved.
            }
            // If page wasn't mapped (not yet demand-faulted), skip — it will
            // be demand-faulted at the new address.
        }

        // Remove old VMA range.
        vm.remove_vma_range(old_addr, old_size)?;

        // Single remote TLB flush for all the moved pages.
        if old_pages > 0 {
            vm.page_table().flush_tlb_remote();
        }

        Ok(new_addr.value() as isize)
    }
}
