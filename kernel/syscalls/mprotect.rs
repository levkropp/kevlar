// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Reference: OSv core/mmu.cc (BSD-3-Clause) — mprotect VMA tracking logic.
// Page table update and TLB flush are arch-specific to Kevlar's x86-64 paging.
use crate::ctypes::MMapProt;
use crate::prelude::*;
use crate::process::current_process;
use crate::syscalls::SyscallHandler;
use kevlar_platform::{address::UserVAddr, arch::PAGE_SIZE};
use kevlar_utils::alignment::is_aligned;

impl<'a> SyscallHandler<'a> {
    pub fn sys_mprotect(
        &mut self,
        addr: UserVAddr,
        len: usize,
        prot: MMapProt,
    ) -> Result<isize> {
        if !is_aligned(addr.value(), PAGE_SIZE) {
            return Err(Errno::EINVAL.into());
        }

        if len == 0 {
            return Ok(0);
        }

        let len = kevlar_utils::alignment::align_up(len, PAGE_SIZE);

        let current = current_process();
        let vm_ref = current.vm();
        let mut vm = vm_ref.as_ref().unwrap().lock();

        // Update VMA protection flags (splits VMAs at boundaries as needed).
        vm.update_prot_range(addr, len, prot)?;

        // Walk the page table and update flags for any already-mapped pages.
        let prot_flags = prot.bits();
        let num_pages = len / PAGE_SIZE;
        for i in 0..num_pages {
            let page_addr = addr.add(i * PAGE_SIZE);
            if vm.page_table_mut().update_page_flags(page_addr, prot_flags) {
                vm.page_table().flush_tlb(page_addr);
            }
        }

        Ok(0)
    }
}
