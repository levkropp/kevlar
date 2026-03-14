// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
// Own implementation based on Linux man pages.
use crate::ctypes::MMapProt;
use crate::prelude::*;
use crate::process::current_process;
use crate::syscalls::SyscallHandler;
use kevlar_platform::{address::UserVAddr, arch::PAGE_SIZE};
use kevlar_utils::alignment::is_aligned;

impl<'a> SyscallHandler<'a> {
    pub fn sys_mprotect(&mut self, addr: UserVAddr, len: usize, prot: MMapProt) -> Result<isize> {
        if !is_aligned(addr.value(), PAGE_SIZE) {
            return Err(Errno::EINVAL.into());
        }

        if len == 0 {
            return Ok(0);
        }

        let len = kevlar_utils::alignment::align_up(len, PAGE_SIZE);

        let current = current_process();
        let vm_ref = current.vm();
        let mut vm = vm_ref.as_ref().unwrap().lock_no_irq();

        // Update VMA protection flags (splits VMAs at boundaries as needed).
        vm.update_prot_range(addr, len, prot)?;

        // Walk the page table and update flags for any already-mapped pages.
        // Use local invlpg per page + a single remote IPI at the end, mirroring
        // the munmap pattern: O(1) IPIs instead of O(pages) IPIs.
        let prot_flags = prot.bits();
        let num_pages = len / PAGE_SIZE;
        let mut any_flushed = false;
        for i in 0..num_pages {
            let page_addr = addr.add(i * PAGE_SIZE);
            if vm.page_table_mut().update_page_flags(page_addr, prot_flags) {
                vm.page_table().flush_tlb_local(page_addr);
                any_flushed = true;
            }
        }
        if any_flushed {
            vm.page_table().flush_tlb_remote();
        }

        Ok(0)
    }
}
