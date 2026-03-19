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
            let mut freed_any = false;
            let mut cursor = addr;
            while cursor < end {
                if let Some(vaddr) = UserVAddr::new(cursor) {
                    if let Some(paddr) = vm.page_table_mut().unmap_user_page(vaddr) {
                        vm.page_table().flush_tlb_local(vaddr);
                        if kevlar_platform::page_refcount::page_ref_dec(paddr) {
                            free_pages(paddr, 1);
                        }
                        freed_any = true;
                    }
                }
                cursor += PAGE_SIZE;
            }

            if freed_any {
                vm.page_table().flush_tlb_remote();
            }

            return Ok(0);
        }

        // All other advice values: accept silently.
        Ok(0)
    }
}
