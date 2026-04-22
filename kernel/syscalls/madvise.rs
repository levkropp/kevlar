// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::prelude::*;
use crate::process::current_process;
use crate::syscalls::SyscallHandler;
use kevlar_platform::{
    address::UserVAddr,
    arch::PAGE_SIZE,
    page_allocator::free_pages,
};
use kevlar_utils::alignment::is_aligned;

const MADV_DONTNEED: i32 = 4;

impl<'a> SyscallHandler<'a> {
    pub fn sys_madvise(&mut self, addr: usize, len: usize, advice: i32) -> Result<isize> {
        if advice == MADV_DONTNEED {
            // MADV_DONTNEED on anonymous pages: unmap pages so next access
            // faults in fresh zeroed pages (demand paging).
            if !is_aligned(addr, PAGE_SIZE) || len == 0 {
                return Ok(0);
            }
            let len = kevlar_utils::alignment::align_up(len, PAGE_SIZE);

            let current = current_process();
            let vm_ref = current.vm();
            let mut vm = vm_ref.as_ref().unwrap().lock_preempt();

            let end = addr + len;
            let mut cleared_any = false;
            let mut to_free: alloc::vec::Vec<kevlar_platform::address::PAddr> =
                alloc::vec::Vec::new();
            let mut cursor = addr;
            while cursor < end {
                if let Some(vaddr) = UserVAddr::new(cursor) {
                    if let Some(paddr) = vm.page_table_mut().unmap_user_page(vaddr) {
                        vm.page_table().flush_tlb_local(vaddr);
                        if kevlar_platform::page_refcount::page_ref_dec(paddr) {
                            to_free.push(paddr);
                        }
                        cleared_any = true;
                    }
                }
                cursor += PAGE_SIZE;
            }

            // Drop the Vm lock BEFORE the cross-CPU TLB flush — same
            // deadlock rationale as munmap (blog 199). Safe because the
            // pages in to_free aren't freed until after the broadcast
            // completes, so stale TLB entries can't resolve to a page
            // belonging to a different process during the window.
            drop(vm);
            if cleared_any {
                kevlar_platform::arch::flush_tlb_remote_all_pcids();
            }
            for paddr in to_free {
                free_pages(paddr, 1);
            }

            return Ok(0);
        }

        // All other advice values: accept silently.
        Ok(0)
    }
}
