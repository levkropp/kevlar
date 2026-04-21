// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
// Own implementation based on Linux man pages.
use crate::ctypes::MMapProt;
use crate::prelude::*;
use crate::process::current_process;
use crate::syscalls::SyscallHandler;
use kevlar_platform::{address::UserVAddr, arch::{PAGE_SIZE, HUGE_PAGE_SIZE}};
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
        let end_value = addr.value() + len;
        let mut any_flushed = false;
        let mut cursor = addr.value();
        while cursor < end_value {
            let page_addr = UserVAddr::new(cursor).unwrap();

            // Check for 2MB huge page only at 2MB-aligned boundaries.
            // Skip the expensive is_huge_mapped() page table walk for
            // non-aligned addresses (the common case for 4KB operations).
            if is_aligned(cursor, HUGE_PAGE_SIZE) {
                if let Some(_) = vm.page_table().is_huge_mapped(page_addr) {
                    if cursor + HUGE_PAGE_SIZE <= end_value {
                        // Full huge page in range — update PDE flags directly.
                        if vm.page_table_mut().update_huge_page_flags(page_addr, prot_flags) {
                            vm.page_table().flush_tlb_local(page_addr);
                            any_flushed = true;
                        }
                        cursor += HUGE_PAGE_SIZE;
                        continue;
                    }
                    // Partial huge page — split into 4KB first.
                    vm.page_table_mut().split_huge_page(page_addr);
                    vm.page_table().flush_tlb_local(page_addr);
                }
            }

            if vm.page_table_mut().update_page_flags(page_addr, prot_flags) {
                vm.page_table().flush_tlb_local(page_addr);
                any_flushed = true;
            }
            cursor += PAGE_SIZE;
        }
        // Drop the Vm lock BEFORE the cross-CPU TLB flush — mirrors the
        // munmap fix (blog 199). A remote CPU's page-fault handler spins
        // on this Vm's lock_no_irq() with IF=0, cannot ACK the TLB
        // shootdown IPI until the lock frees. Dropping first breaks the
        // deadlock. Safe: PTE flag updates are already committed to our
        // page table (local invlpg'd); remote CPUs still have stale
        // flags until the broadcast completes, matching Linux's
        // non-atomic-across-CPUs mprotect semantics.
        drop(vm);
        if any_flushed {
            kevlar_platform::arch::flush_tlb_remote_all_pcids();
        }

        Ok(0)
    }
}
