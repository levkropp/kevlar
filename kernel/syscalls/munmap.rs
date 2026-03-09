// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Reference: OSv core/mmu.cc (BSD-3-Clause) — munmap VMA removal logic.
// Page table unmapping and TLB flush are arch-specific to Kevlar's x86-64 paging.
use crate::prelude::*;
use crate::process::current_process;
use crate::syscalls::SyscallHandler;
use kevlar_platform::{
    address::UserVAddr,
    arch::PAGE_SIZE,
    page_allocator::free_pages,
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
        let vm_ref = current.vm();
        let mut vm = vm_ref.as_ref().unwrap().lock_no_irq();

        // Remove VMAs in the range (splits at boundaries).
        vm.remove_vma_range(addr, len)?;

        // Walk the page table: unmap PTEs and free physical pages.
        let num_pages = len / PAGE_SIZE;
        for i in 0..num_pages {
            let page_addr = addr.add(i * PAGE_SIZE);
            if let Some(paddr) = vm.page_table_mut().unmap_user_page(page_addr) {
                // Free the physical page.
                free_pages(paddr, 1);
                vm.page_table().flush_tlb(page_addr);
            }
        }

        Ok(0)
    }
}
