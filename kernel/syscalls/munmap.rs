// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
// Own implementation based on Linux man pages.
use crate::prelude::*;
use crate::process::current_process;
use crate::syscalls::SyscallHandler;
use kevlar_platform::{
    address::{PAddr, UserVAddr},
    arch::{PAGE_SIZE, HUGE_PAGE_SIZE},
    page_allocator::{free_pages, free_huge_page_and_zero},
};
use kevlar_utils::alignment::is_aligned;

impl<'a> SyscallHandler<'a> {
    pub fn sys_munmap(&mut self, addr: UserVAddr, len: usize) -> Result<isize> {
        if !is_aligned(addr.value(), PAGE_SIZE) {
            return Err(Errno::EINVAL.into());
        }

        if len == 0 {
            return Err(Errno::EINVAL.into());
        }

        let len = kevlar_utils::alignment::align_up(len, PAGE_SIZE);

        let current = current_process();
        kevlar_platform::flight_recorder::record(
            kevlar_platform::flight_recorder::kind::MUNMAP,
            current.pid().as_i32() as u32,
            addr.value() as u64,
            len as u64,
        );
        let vm_ref = current.vm();
        // lock_preempt: keeps IF=1 (remote CPUs can ACK TLB shootdown IPIs)
        // but disables preemption (prevents the timer from calling switch() on
        // this CPU while we hold the lock, which would deadlock trying to
        // re-acquire the same SpinMutex).
        let mut vm = vm_ref.as_ref().unwrap().lock_preempt();

        // Remove VMAs in the range (splits at boundaries).
        vm.remove_vma_range(addr, len)?;

        // Walk the page table: clear PTEs and collect physical pages.
        // We do local invlpg per page here, then ONE remote TLB flush IPI
        // for the entire range (instead of one IPI per page), reducing
        // IPI overhead from O(pages) to O(1) per sys_munmap call.
        let end_value = addr.value() + len;
        // Track pages to free: (paddr, num_pages) pairs.
        let mut to_free: alloc::vec::Vec<(PAddr, usize)> = alloc::vec::Vec::new();
        let mut cursor = addr.value();
        while cursor < end_value {
            let page_addr = UserVAddr::new(cursor).unwrap();

            // Check for 2MB huge page at this address.
            if is_aligned(cursor, HUGE_PAGE_SIZE) && cursor + HUGE_PAGE_SIZE <= end_value {
                if let Some(hp_paddr) = vm.page_table_mut().unmap_huge_user_page(page_addr) {
                    vm.page_table().flush_tlb_local(page_addr);
                    // Check if all 512 sub-pages are sole-owner (refcount 1).
                    // If so, we can bulk-zero and pool the entire 2MB page.
                    let mut all_sole_owner = true;
                    for sub_i in 0..512usize {
                        let sub = PAddr::new(hp_paddr.value() + sub_i * PAGE_SIZE);
                        if kevlar_platform::page_refcount::page_ref_count(sub) != 1 {
                            all_sole_owner = false;
                            break;
                        }
                    }
                    if all_sole_owner {
                        // Marker: (base_paddr, 512) — handled specially in free loop.
                        to_free.push((hp_paddr, 512));
                    } else {
                        // Divergent refcounts from CoW — free sub-pages individually.
                        for sub_i in 0..512usize {
                            to_free.push((
                                PAddr::new(hp_paddr.value() + sub_i * PAGE_SIZE),
                                1,
                            ));
                        }
                    }
                    cursor += HUGE_PAGE_SIZE;
                    continue;
                }
            }

            // If this address is inside a huge page but not 2MB-aligned (or
            // the unmap range doesn't cover the full 2MB), split first.
            if vm.page_table().is_huge_mapped(page_addr).is_some() {
                vm.page_table_mut().split_huge_page(page_addr);
                vm.page_table().flush_tlb_local(page_addr);
                // After splitting, fall through to 4KB unmap below.
            }

            if let Some(paddr) = vm.page_table_mut().unmap_user_page(page_addr) {
                vm.page_table().flush_tlb_local(page_addr);
                to_free.push((paddr, 1));
            }
            cursor += PAGE_SIZE;
        }

        // One batch remote TLB flush: remote CPUs reload CR3.
        // Must happen BEFORE freeing pages so no CPU can write through a
        // stale TLB entry to a page that has been returned to the allocator.
        if !to_free.is_empty() {
            vm.page_table().flush_tlb_remote();
        }

        // Now safe to free all unmapped physical pages.
        // CoW: only free when refcount drops to 0.
        for (paddr, num) in to_free {
            if num == 512 {
                // Sole-owner huge page: bulk dec refcounts, zero, and pool.
                for sub_i in 0..512usize {
                    let sub = PAddr::new(paddr.value() + sub_i * PAGE_SIZE);
                    kevlar_platform::page_refcount::page_ref_dec(sub);
                }
                free_huge_page_and_zero(paddr);
            } else if kevlar_platform::page_refcount::page_ref_dec(paddr) {
                free_pages(paddr, num);
            }
        }

        Ok(0)
    }
}
